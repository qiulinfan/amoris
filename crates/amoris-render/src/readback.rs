//! Map-to-future bridge shared by complete-field and small-summary readbacks. Mapping may call
//! back synchronously on failure; cancellation must unmap without holding callback state.
use std::sync::{Arc, Mutex};

/// Arm every independent buffer before awaiting any callback. Dropping this future cancels all
/// its outstanding maps, including when another map or the device wait fails.
pub(crate) async fn read_many<T: bytemuck::Pod>(
    requests: &mut [(&wgpu::Buffer, &mut [T])],
) -> Result<(), String> {
    for (buffer, out) in requests.iter() {
        let bytes = std::mem::size_of_val(*out) as u64;
        if bytes == 0 || !bytes.is_multiple_of(4) || bytes > buffer.size() {
            return Err(
                "readback length must fit its buffer and be a nonempty multiple of four bytes"
                    .into(),
            );
        }
    }
    if requests.is_empty() {
        return Ok(());
    }
    struct Completion {
        remaining: usize,
        error: Option<String>,
        waker: Option<std::task::Waker>,
    }
    struct Mapping(wgpu::Buffer);
    impl Drop for Mapping {
        fn drop(&mut self) {
            self.0.unmap();
        }
    }
    let mappings: Vec<_> = requests
        .iter()
        .map(|(buffer, _)| Mapping((*buffer).clone()))
        .collect();
    let completion = Arc::new(Mutex::new(Completion {
        remaining: requests.len(),
        error: None,
        waker: None,
    }));
    for (buffer, out) in requests.iter() {
        let bytes = std::mem::size_of_val(*out) as u64;
        let callback = completion.clone();
        // Do not hold completion's mutex here: a refused map may call back synchronously.
        buffer
            .slice(..bytes)
            .map_async(wgpu::MapMode::Read, move |result| {
                let wake = {
                    let mut c = callback
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    c.remaining -= 1;
                    if let Err(error) = result {
                        c.error
                            .get_or_insert_with(|| format!("GPU readback map failed: {error}"));
                    }
                    c.waker.take()
                };
                if let Some(wake) = wake {
                    wake.wake();
                }
            });
    }
    std::future::poll_fn(|cx| {
        let mut c = completion
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(error) = c.error.take() {
            std::task::Poll::Ready(Err(error))
        } else if c.remaining == 0 {
            std::task::Poll::Ready(Ok(()))
        } else {
            c.waker = Some(cx.waker().clone());
            std::task::Poll::Pending
        }
    })
    .await?;
    for (buffer, out) in requests.iter_mut() {
        let bytes = std::mem::size_of_val(*out) as u64;
        let view = buffer
            .slice(..bytes)
            .get_mapped_range()
            .map_err(|e| format!("GPU mapped range failed: {e}"))?;
        let data = bytemuck::try_cast_slice(&view)
            .map_err(|e| format!("GPU readback layout failed: {e}"))?;
        out.copy_from_slice(data);
    }
    drop(mappings);
    Ok(())
}

pub(crate) async fn read<T: bytemuck::Pod>(
    buffer: &wgpu::Buffer,
    out: &mut [T],
) -> Result<(), String> {
    read_many(&mut [(buffer, out)]).await
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn read_many_blocking<T: bytemuck::Pod>(
    device: &wgpu::Device,
    requests: &mut [(&wgpu::Buffer, &mut [T])],
) -> Result<(), String> {
    use std::future::Future;
    let mut future = Box::pin(read_many(requests));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    if let std::task::Poll::Ready(result) = future.as_mut().poll(&mut cx) {
        return result;
    }
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .map_err(|e| format!("GPU readback wait failed: {e}"))?;
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => {
            Err("GPU readback callback did not complete after its submission".into())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn read_blocking<T: bytemuck::Pod>(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    out: &mut [T],
) -> Result<(), String> {
    read_many_blocking(device, &mut [(buffer, out)])
}
