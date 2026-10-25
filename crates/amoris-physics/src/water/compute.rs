//! Host-provided water computation, without a graphics dependency in the game.

pub use super::feedback::ReactionBudget;
pub use super::resident::{
    BoundaryFlux, ResidentOutput, ResidentRequest, ResidentResult, ResidentStats,
};
use amoris_contract::Problem;
use amoris_sim::Sim;
use bevy_ecs::prelude::Resource;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

/// The gathered window, in rows. `u` is (width + 1) * height; `w` is width * (height + 1).
/// Inactive columns and faces bordering them must remain unchanged; outer faces are walls.
pub struct GridView<'a> {
    pub size: [usize; 2],
    pub bed: &'a [f64],
    pub depth: &'a mut [f64],
    pub u: &'a mut [f64],
    pub w: &'a mut [f64],
    pub active: &'a [bool],
}

/// All substeps of one tick. No result may depend on a previous call's hidden state.
pub trait WaterCompute: Send + Sync {
    fn poll_resident_batch(
        &mut self,
        _requests: &[ResidentRequest<'_>],
    ) -> Result<std::task::Poll<Vec<ResidentResult>>, Problem> {
        Err(Problem::new(
            "water.resident_unavailable",
            "This host has no resident GPU execution service",
            amoris_contract::detail([]),
        ))
    }
    /// `damping`: bed friction (m/s) and calm damping (per second; water.md 4).
    fn advance(
        &mut self,
        grid: GridView<'_>,
        cell_m: f64,
        substep_dt: f64,
        substeps: u32,
        damping: [f64; 2],
    ) -> Result<(), Problem>;

    /// An asynchronous host returns Pending without modifying the output. It later completes
    /// its owned request; the same system polls again, without reapplying sources or beds.
    fn poll_advance(
        &mut self,
        grid: GridView<'_>,
        cell_m: f64,
        substep_dt: f64,
        substeps: u32,
        damping: [f64; 2],
    ) -> Result<std::task::Poll<()>, Problem> {
        self.advance(grid, cell_m, substep_dt, substeps, damping)?;
        Ok(std::task::Poll::Ready(()))
    }

    fn invalidate(&mut self) {}
}

#[derive(Resource)]
pub(crate) struct Backend(pub Box<dyn WaterCompute>);

/// Install through `GameBuilder::system`, so forks install their own service too.
pub fn install(sim: &mut Sim, backend: impl WaterCompute + 'static) {
    sim.world_mut().insert_resource(Backend(Box::new(backend)));
}

/// Called after replacing a world: no old suspended request or continuation may enter its tick.
pub fn invalidate(world: &mut bevy_ecs::world::World) {
    if let Some(mut backend) = world.get_resource_mut::<Backend>() {
        backend.0.invalidate();
    }
    if let Some(mut scratch) = world.get_resource_mut::<super::WaterScratch>() {
        *scratch = Default::default();
    }
    if let Some(mut steps) = world.get_resource_mut::<super::WaterSteps>() {
        *steps = Default::default();
    }
    if let Some(mut samples) = world.get_resource_mut::<super::buoyancy::Samples>() {
        *samples = Default::default();
    }
    if let Some(mut samples) = world.get_resource_mut::<super::swimming::Samples>() {
        *samples = Default::default();
    }
}

/// CPU-owned input transferred to an asynchronous host. No world borrow survives suspension.
#[derive(Debug)]
pub struct Request {
    pub id: u64,
    pub size: [usize; 2],
    pub bed: Vec<f64>,
    pub depth: Vec<f64>,
    pub u: Vec<f64>,
    pub w: Vec<f64>,
    pub active: Vec<bool>,
    pub cell_m: f64,
    pub dt: f64,
    pub substeps: u32,
    pub friction: f64,
    pub calm_per_s: f64,
}

/// Row-major depth and minus-face velocities. Beds remain exactly the world's authored values.
#[derive(Debug)]
pub struct Output {
    pub columns: Vec<[f64; 3]>,
}

/// An owned resident continuation. Checkpoint words are immutable and shared with the world;
/// queuing a normal tick never clones the full field.
#[derive(Debug)]
pub struct ResidentJob {
    pub region: amoris_sim::EntityId,
    pub checkpoint: Arc<amoris_assets::water_field::CheckedCheckpoint>,
    pub recipe: amoris_assets::water_field::FieldRecipe,
    pub step: Option<amoris_assets::water_field::FieldStep>,
    pub points: Vec<[f64; 2]>,
    pub checkpoint_now: bool,
    pub coupled_from: Option<u32>,
}
impl ResidentJob {
    pub fn view(&self) -> ResidentRequest<'_> {
        ResidentRequest {
            region: self.region,
            checkpoint: self.checkpoint.clone(),
            recipe: &self.recipe,
            step: self.step.as_ref(),
            points: &self.points,
            checkpoint_now: self.checkpoint_now,
            coupled_from: self.coupled_from,
        }
    }
}
#[derive(Debug)]
pub struct ResidentBatchJob {
    pub id: u64,
    pub requests: Vec<ResidentJob>,
}
#[derive(Debug)]
pub enum Job {
    Grid(Request),
    Resident(Box<ResidentBatchJob>),
}
impl Job {
    pub fn id(&self) -> u64 {
        match self {
            Self::Grid(r) => r.id,
            Self::Resident(r) => r.id,
        }
    }
}
#[derive(Debug)]
pub enum JobOutput {
    Grid(Output),
    Resident(Vec<ResidentResult>),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Grid,
    Resident,
}
impl JobOutput {
    fn kind(&self) -> JobKind {
        match self {
            Self::Grid(_) => JobKind::Grid,
            Self::Resident(_) => JobKind::Resident,
        }
    }
}

#[derive(Default)]
struct QueueState {
    next: u64,
    requests: VecDeque<Job>,
    live: BTreeMap<u64, JobKind>,
    responses: BTreeMap<u64, Result<JobOutput, Problem>>,
}

/// Four outstanding grid/resident jobs combined; each resident job is one bounded cohort.
/// Responses not yet consumed still occupy their job's slot. Hosts
/// consume jobs and complete their tokens; callbacks from cancelled worlds are ignored.
#[derive(Clone, Default)]
pub struct ComputeQueue(Arc<Mutex<QueueState>>);

fn failed(message: impl Into<String>) -> Problem {
    Problem::new(
        "water.compute_failed",
        message.into(),
        amoris_contract::detail([]),
    )
}
impl ComputeQueue {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, QueueState>, Problem> {
        self.0
            .lock()
            .map_err(|_| failed("water compute queue was poisoned"))
    }
    pub fn backend(&self) -> impl WaterCompute + use<> {
        Queued {
            queue: self.clone(),
            pending: None,
        }
    }
    pub fn take_request(&self) -> Result<Option<Request>, Problem> {
        let mut q = self.lock()?;
        if matches!(q.requests.front(), Some(Job::Grid(_))) {
            return Ok(q.requests.pop_front().and_then(|r| match r {
                Job::Grid(r) => Some(r),
                _ => None,
            }));
        }
        Ok(None)
    }
    pub fn take_job(&self) -> Result<Option<Job>, Problem> {
        Ok(self.lock()?.requests.pop_front())
    }
    pub fn complete(&self, id: u64, result: Result<Output, Problem>) -> Result<bool, Problem> {
        self.complete_job(id, result.map(JobOutput::Grid))
    }
    pub fn complete_resident(
        &self,
        id: u64,
        result: Result<Vec<ResidentResult>, Problem>,
    ) -> Result<bool, Problem> {
        self.complete_job(id, result.map(JobOutput::Resident))
    }
    pub fn complete_job(
        &self,
        id: u64,
        mut result: Result<JobOutput, Problem>,
    ) -> Result<bool, Problem> {
        let mut q = self.lock()?;
        let Some(kind) = q.live.get(&id) else {
            return Ok(false);
        };
        if q.responses.contains_key(&id) {
            return Ok(false);
        }
        if result.as_ref().is_ok_and(|out| out.kind() != *kind) {
            result = Err(failed(
                "water response kind differs from the requested computation",
            ));
        }
        q.responses.insert(id, result);
        Ok(true)
    }
    fn cancel(&self, id: u64) {
        if let Ok(mut q) = self.0.lock() {
            q.live.remove(&id);
            q.responses.remove(&id);
            q.requests.retain(|r| r.id() != id);
        }
    }
}

struct Queued {
    queue: ComputeQueue,
    pending: Option<(u64, JobKind)>,
}
impl Drop for Queued {
    fn drop(&mut self) {
        self.invalidate();
    }
}
impl WaterCompute for Queued {
    fn advance(
        &mut self,
        _: GridView<'_>,
        _: f64,
        _: f64,
        _: u32,
        _: [f64; 2],
    ) -> Result<(), Problem> {
        Err(failed(
            "asynchronous water needs poll_step and host request completion",
        ))
    }
    fn poll_advance(
        &mut self,
        grid: GridView<'_>,
        cell_m: f64,
        dt: f64,
        substeps: u32,
        [friction, calm_per_s]: [f64; 2],
    ) -> Result<std::task::Poll<()>, Problem> {
        let mut q = self.queue.lock()?;
        if let Some((id, kind)) = self.pending {
            if kind != JobKind::Grid {
                return Err(failed("a resident computation is already pending"));
            }
            let Some(result) = q.responses.remove(&id) else {
                return Ok(std::task::Poll::Pending);
            };
            q.live.remove(&id);
            self.pending = None;
            let output = match result? {
                JobOutput::Grid(out) => out,
                JobOutput::Resident(_) => {
                    return Err(failed("resident result for a grid computation"));
                }
            };
            let [nx, nz] = grid.size;
            if output.columns.len() != nx * nz
                || output
                    .columns
                    .iter()
                    .any(|c| c[0] < 0.0 || c.iter().any(|v| !v.is_finite()))
            {
                return Err(failed(
                    "water compute output has invalid dimensions, depth or velocity",
                ));
            }
            for k in 0..nz {
                for i in 0..nx {
                    let n = i + nx * k;
                    let c = output.columns[n];
                    if (i == 0 && c[1] != 0.0)
                        || (k == 0 && c[2] != 0.0)
                        || (i > 0 && grid.active[n] != grid.active[n - 1] && c[1] != 0.0)
                        || (k > 0 && grid.active[n] != grid.active[n - nx] && c[2] != 0.0)
                    {
                        return Err(failed("water compute output crosses a wall"));
                    }
                }
            }
            for k in 0..nz {
                for i in 0..nx {
                    let n = i + nx * k;
                    let c = output.columns[n];
                    grid.depth[n] = c[0];
                    grid.u[i + (nx + 1) * k] = c[1];
                    grid.w[n] = c[2];
                }
            }
            return Ok(std::task::Poll::Ready(()));
        }
        if q.live.len() >= 4 {
            return Ok(std::task::Poll::Pending);
        }
        let id = q.next;
        q.next = id
            .checked_add(1)
            .ok_or_else(|| failed("water request tokens exhausted"))?;
        q.requests.push_back(Job::Grid(Request {
            id,
            size: grid.size,
            bed: grid.bed.to_vec(),
            depth: grid.depth.to_vec(),
            u: grid.u.to_vec(),
            w: grid.w.to_vec(),
            active: grid.active.to_vec(),
            cell_m,
            dt,
            substeps,
            friction,
            calm_per_s,
        }));
        q.live.insert(id, JobKind::Grid);
        self.pending = Some((id, JobKind::Grid));
        Ok(std::task::Poll::Pending)
    }
    fn poll_resident_batch(
        &mut self,
        requests: &[ResidentRequest<'_>],
    ) -> Result<std::task::Poll<Vec<ResidentResult>>, Problem> {
        super::resident::validate_batch(requests)?;
        let mut q = self.queue.lock()?;
        if let Some((id, kind)) = self.pending {
            if kind != JobKind::Resident {
                return Err(failed("a grid computation is already pending"));
            }
            let Some(result) = q.responses.remove(&id) else {
                return Ok(std::task::Poll::Pending);
            };
            q.live.remove(&id);
            self.pending = None;
            return match result? {
                JobOutput::Resident(out) => {
                    super::resident::validate_results(requests, &out)?;
                    Ok(std::task::Poll::Ready(out))
                }
                JobOutput::Grid(_) => Err(failed("grid result for a resident computation")),
            };
        }
        if q.live.len() >= 4 {
            return Ok(std::task::Poll::Pending);
        }
        let id = q.next;
        q.next = id
            .checked_add(1)
            .ok_or_else(|| failed("water request tokens exhausted"))?;
        let requests = requests
            .iter()
            .map(|r| ResidentJob {
                region: r.region,
                checkpoint: r.checkpoint.clone(),
                recipe: r.recipe.clone(),
                step: r.step.cloned(),
                points: r.points.to_vec(),
                coupled_from: r.coupled_from,
                checkpoint_now: r.checkpoint_now,
            })
            .collect();
        q.requests
            .push_back(Job::Resident(Box::new(ResidentBatchJob { id, requests })));
        q.live.insert(id, JobKind::Resident);
        self.pending = Some((id, JobKind::Resident));
        Ok(std::task::Poll::Pending)
    }
    fn invalidate(&mut self) {
        if let Some((id, _)) = self.pending.take() {
            self.queue.cancel(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn poll(
        b: &mut dyn WaterCompute,
        depth: &mut [f64],
        u: &mut [f64],
        w: &mut [f64],
    ) -> Result<std::task::Poll<()>, Problem> {
        b.poll_advance(
            GridView {
                size: [2, 2],
                bed: &[0.0; 4],
                depth,
                u,
                w,
                active: &[true; 4],
            },
            1.0,
            1.0 / 120.0,
            2,
            [0.02, 0.2],
        )
    }
    #[test]
    fn cancelled_tokens_are_not_reused_and_invalid_responses_never_partially_commit() {
        let q = ComputeQueue::default();
        let mut b = q.backend();
        let (mut d, mut u, mut w) = ([1.0; 4], [0.0; 6], [0.0; 6]);
        assert!(poll(&mut b, &mut d, &mut u, &mut w).unwrap().is_pending());
        let old = q.take_request().unwrap().unwrap();
        b.invalidate();
        assert!(
            !q.complete(
                old.id,
                Ok(Output {
                    columns: vec![[2.0, 0.0, 0.0]; 4]
                })
            )
            .unwrap()
        );
        assert!(poll(&mut b, &mut d, &mut u, &mut w).unwrap().is_pending());
        let fresh = q.take_request().unwrap().unwrap();
        assert!(fresh.id > old.id);
        let mut columns = vec![[2.0, 0.0, 0.0]; 4];
        columns[3][0] = f64::NAN;
        assert!(q.complete(fresh.id, Ok(Output { columns })).unwrap());
        assert!(
            !q.complete(
                fresh.id,
                Ok(Output {
                    columns: vec![[2.0, 0.0, 0.0]; 4]
                })
            )
            .unwrap()
        );
        assert_eq!(
            poll(&mut b, &mut d, &mut u, &mut w).unwrap_err().code,
            "water.compute_failed"
        );
        assert_eq!(d, [1.0; 4]);
        assert_eq!(u, [0.0; 6]);
        assert_eq!(w, [0.0; 6]);
    }
    #[test]
    fn queued_and_completed_but_unconsumed_jobs_share_the_outstanding_limit() {
        let q = ComputeQueue::default();
        let mut backends: Vec<_> = (0..5).map(|_| q.backend()).collect();
        let (mut d, mut u, mut w) = ([1.0; 4], [0.0; 6], [0.0; 6]);
        for b in &mut backends {
            assert!(poll(b, &mut d, &mut u, &mut w).unwrap().is_pending());
        }
        let jobs: Vec<_> = std::iter::from_fn(|| q.take_request().unwrap()).collect();
        assert_eq!(jobs.len(), 4);
        q.complete(
            jobs[0].id,
            Ok(Output {
                columns: vec![[1.0, 0.0, 0.0]; 4],
            }),
        )
        .unwrap();
        assert!(
            poll(&mut backends[4], &mut d, &mut u, &mut w)
                .unwrap()
                .is_pending()
        );
        assert!(q.take_request().unwrap().is_none());
        assert!(
            poll(&mut backends[0], &mut d, &mut u, &mut w)
                .unwrap()
                .is_ready()
        );
        assert!(
            poll(&mut backends[4], &mut d, &mut u, &mut w)
                .unwrap()
                .is_pending()
        );
        assert_eq!(q.take_request().unwrap().unwrap().id, 4);
    }
}

#[cfg(test)]
mod resident_tests {
    use super::*;
    use amoris_assets::water_field::{FieldCheckpoint, FieldRecipe};

    #[test]
    fn resident_and_grid_jobs_share_limits_and_validate_reply_kind_and_cancellation() {
        let q = ComputeQueue::default();
        let seed = Arc::new(
            FieldCheckpoint::new([2, 2], 0.0, vec![[0, 1.0f32.to_bits(), 0, 0]; 4])
                .checked()
                .unwrap(),
        );
        let recipe = FieldRecipe::new(seed.key(), vec![], seed.digest(), String::new());
        let request = || ResidentRequest {
            region: amoris_sim::EntityId::FIRST,
            checkpoint: seed.clone(),
            recipe: &recipe,
            step: None,
            points: &[[0.5, 0.5]],
            coupled_from: None,
            checkpoint_now: false,
        };
        let mut backends: Vec<_> = (0..5).map(|_| q.backend()).collect();
        let (mut d, mut u, mut w) = ([1.0; 4], [0.0; 6], [0.0; 6]);
        let grid = GridView {
            size: [2, 2],
            bed: &[0.0; 4],
            depth: &mut d,
            u: &mut u,
            w: &mut w,
            active: &[true; 4],
        };
        assert!(
            backends[0]
                .poll_advance(grid, 1.0, 1.0 / 120.0, 2, [0.02, 0.2])
                .unwrap()
                .is_pending()
        );
        for b in &mut backends[1..] {
            assert!(b.poll_resident_batch(&[request()]).unwrap().is_pending());
        }
        let jobs: Vec<_> = std::iter::from_fn(|| q.take_job().unwrap()).collect();
        assert_eq!(jobs.len(), 4);
        for job in &jobs[1..] {
            let Job::Resident(r) = job else {
                panic!("resident job")
            };
            assert!(
                Arc::ptr_eq(&seed, &r.requests[0].checkpoint),
                "queuing must not clone the full field"
            );
        }
        assert!(
            q.complete(
                jobs[1].id(),
                Ok(Output {
                    columns: vec![[0.0; 3]; 4]
                })
            )
            .unwrap()
        );
        assert_eq!(
            backends[1]
                .poll_resident_batch(&[request()])
                .unwrap_err()
                .code,
            "water.compute_failed"
        );
        assert!(
            backends[4]
                .poll_resident_batch(&[request()])
                .unwrap()
                .is_pending()
        );
        let new = q.take_job().unwrap().unwrap();
        assert_eq!(new.id(), 4);
        backends[4].invalidate();
        assert!(
            !q.complete_job(new.id(), Err(failed("cancelled host")))
                .unwrap()
        );
        assert!(
            backends[4]
                .poll_resident_batch(&[request()])
                .unwrap()
                .is_pending()
        );
        let fresh = q.take_job().unwrap().unwrap();
        assert!(fresh.id() > new.id());
        // A grid-only consumer must not silently discard a resident request at the front.
        backends[4].invalidate();
        assert!(
            backends[4]
                .poll_resident_batch(&[request()])
                .unwrap()
                .is_pending()
        );
        assert!(q.take_request().unwrap().is_none());
        assert!(matches!(q.take_job().unwrap(), Some(Job::Resident(_))));
    }
    fn seed(size: [u32; 2]) -> Arc<amoris_assets::water_field::CheckedCheckpoint> {
        Arc::new(
            FieldCheckpoint::new(
                size,
                0.0,
                vec![[0, 1.0f32.to_bits(), 0, 0]; size[0] as usize * size[1] as usize],
            )
            .checked()
            .unwrap(),
        )
    }
    fn tagged(region: u64, recipe: &FieldRecipe) -> ResidentResult {
        ResidentResult {
            region: amoris_sim::EntityId::new(region).unwrap(),
            output: ResidentOutput {
                recipe: recipe.clone(),
                stats: Default::default(),
                samples: vec![],
                checkpoint: None,
                reaction: Default::default(),
                boundary: Default::default(),
                edges: vec![],
            },
        }
    }
    #[test]
    fn batches_reject_empty_duplicate_over_budget_and_wrong_order_before_consumption() {
        let q = ComputeQueue::default();
        let mut backend = q.backend();
        let seed = seed([2, 2]);
        let recipe = FieldRecipe::new(seed.key(), vec![], seed.digest(), String::new());
        let request = |id| ResidentRequest {
            region: amoris_sim::EntityId::new(id).unwrap(),
            checkpoint: seed.clone(),
            recipe: &recipe,
            step: None,
            points: &[],
            checkpoint_now: false,
            coupled_from: None,
        };
        assert!(backend.poll_resident_batch(&[]).is_err());
        assert!(
            backend
                .poll_resident_batch(&[request(1), request(1)])
                .is_err()
        );
        assert!(
            backend
                .poll_resident_batch(&(1..=17).map(request).collect::<Vec<_>>())
                .is_err()
        );
        assert!(q.take_job().unwrap().is_none());
        let points = vec![[0.5; 2]; amoris_assets::water_field::MAX_SAMPLES + 1];
        let mut oversized = request(1);
        oversized.points = &points;
        assert!(backend.poll_resident_batch(&[oversized]).is_err());
        let mut invalid_split = request(1);
        invalid_split.coupled_from = Some(1);
        assert!(backend.poll_resident_batch(&[invalid_split]).is_err());
        assert!(q.take_job().unwrap().is_none());
        let inputs = [request(1), request(2)];
        assert!(backend.poll_resident_batch(&inputs).unwrap().is_pending());
        let job = q.take_job().unwrap().unwrap();
        q.complete_resident(job.id(), Ok(vec![tagged(2, &recipe), tagged(1, &recipe)]))
            .unwrap();
        assert_eq!(
            backend.poll_resident_batch(&inputs).unwrap_err().code,
            "water.resident_failed"
        );
        assert!(backend.poll_resident_batch(&inputs).unwrap().is_pending());
        let job = q.take_job().unwrap().unwrap();
        q.complete_resident(job.id(), Ok(vec![tagged(1, &recipe)]))
            .unwrap();
        assert!(backend.poll_resident_batch(&inputs).is_err());

        let seed = self::seed([512, 512]);
        let recipe = FieldRecipe::new(seed.key(), vec![], seed.digest(), String::new());
        let requests: Vec<_> = (1..=5)
            .map(|id| ResidentRequest {
                region: amoris_sim::EntityId::new(id).unwrap(),
                checkpoint: seed.clone(),
                recipe: &recipe,
                step: None,
                points: &[],
                checkpoint_now: false,
                coupled_from: None,
            })
            .collect();
        assert!(backend.poll_resident_batch(&requests).is_err());
        assert!(
            backend
                .poll_resident_batch(&requests[..4])
                .unwrap()
                .is_pending()
        );
        backend.invalidate();
        assert!(q.take_job().unwrap().is_none());
    }
    #[test]
    fn cancellation_and_drop_reject_every_late_batch_callback() {
        let q = ComputeQueue::default();
        let seed = seed([2, 2]);
        let recipe = FieldRecipe::new(seed.key(), vec![], seed.digest(), String::new());
        let inputs: Vec<_> = (1..=4)
            .map(|id| ResidentRequest {
                region: amoris_sim::EntityId::new(id).unwrap(),
                checkpoint: seed.clone(),
                recipe: &recipe,
                step: None,
                points: &[],
                checkpoint_now: false,
                coupled_from: None,
            })
            .collect();
        let mut backend = q.backend();
        assert!(backend.poll_resident_batch(&inputs).unwrap().is_pending());
        let old = q.take_job().unwrap().unwrap();
        backend.invalidate();
        assert!(
            !q.complete_resident(
                old.id(),
                Ok((1..=4).map(|id| tagged(id, &recipe)).collect())
            )
            .unwrap()
        );
        assert!(
            !q.complete_resident(old.id(), Err(failed("late GPU map failure")))
                .unwrap()
        );
        assert!(backend.poll_resident_batch(&inputs).unwrap().is_pending());
        let fresh = q.take_job().unwrap().unwrap();
        assert!(fresh.id() > old.id());
        drop(backend);
        assert!(
            !q.complete_resident(
                fresh.id(),
                Ok((1..=4).map(|id| tagged(id, &recipe)).collect())
            )
            .unwrap()
        );
        assert!(q.take_job().unwrap().is_none());
    }
}
