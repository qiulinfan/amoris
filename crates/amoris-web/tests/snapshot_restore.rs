//! Browser kept-snapshot restoration preserves hash tails even while publication is blocked.
use amoris_runtime::{Game, Project};
use amoris_web::{
    Pacing,
    presenter::PresenterCore,
    worker::{Next, Out, WorkerCore},
};
use serde_json::{Value, json};
use std::sync::Arc;
fn cmd(seq: u64, name: &str, params: Value) -> Value {
    json!({"t":"cmd","source":{"developer":0},"seq":seq,"name":name,"params":params.to_string()})
}
#[test]
fn restore_under_backpressure_retains_both_epochs_and_restarts_the_render_feed() {
    std::thread::Builder::new().stack_size(amoris_runtime::GAME_STACK_BYTES).spawn(|| {
        let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/anim");
        let setup=Arc::new(Project::load(&root).unwrap().setup(false).unwrap());
        let mut w=WorkerCore::new(Game::new(setup,1).unwrap(),Pacing::Stepped,Arc::new(||0.0)).unwrap();
        let mut out=w.take_out();
        let feed=amoris_assets::Feed::new();let mailbox=feed.subscribe();let mut ex=amoris_runtime::Extractor::default();
        w.present(&mut ex,&feed);assert!(mailbox.take().reset);
        w.push(&cmd(1,"step",json!({"ticks":130})));
        assert_eq!(w.run(8.0),Next::Command);out.extend(w.take_out());
        assert_eq!(w.game().tick().0,130);
        w.push(&cmd(2,"snapshots.list",json!({})));w.run(8.0);
        let list=w.take_out();let snaps=list.iter().find_map(|o|match o {Out::Message(m) if m["seq"]==2=>Some(m["ok"]["snapshots"].clone()),_=>None}).unwrap();
        assert_eq!(snaps.as_array().unwrap().iter().map(|s|s["tick"].as_u64().unwrap()).collect::<Vec<_>>(),vec![0,60,120]);
        let expected=snaps[1]["world_hash"].clone();
        w.push(&cmd(3,"snapshots.restore",json!({"tick":67})));w.run(8.0);
        let restored=w.take_out();let reply=restored.iter().find_map(|o|match o {Out::Message(m) if m["seq"]==3=>Some(m),_=>None}).unwrap();
        assert_eq!(reply["ok"]["restored"],60);assert_eq!(reply["ok"]["epoch"],1);assert_eq!(reply["ok"]["world_hash"],expected);
        assert!(restored.iter().all(|o|!matches!(o,Out::Snap(..))),"restore must respect two outstanding snapshots");
        w.present(&mut ex,&feed);assert!(mailbox.take().reset,"restore must rebuild the visual cache");
        w.ack(1);out.extend(w.take_out());
        w.ack(2);w.ack(3);
        w.push(&cmd(4,"step",json!({"ticks":5})));w.run(8.0);out.extend(w.take_out());
        w.ack(4);out.extend(w.take_out());
        let mut p=PresenterCore::new();let mut hashes=Vec::new();
        for item in out {if let Out::Snap(meta,bytes)=item {
            p.receive(&meta,&bytes).unwrap();
            hashes.extend(meta["hashes"].as_array().unwrap().iter().zip(meta["hash_epochs"].as_array().unwrap()).map(|(h,e)|(e.as_u64().unwrap(),h[0].as_u64().unwrap())));
        }}
        let expected:Vec<_>=(0..=130).map(|t|(0,t)).chain((61..=65).map(|t|(1,t))).collect();
        assert_eq!(hashes,expected);assert!(p.problems().is_empty(),"{:?}",p.problems());
        assert_eq!(p.latest().unwrap().snapshot.header().tick.0,65);
        w.push(&cmd(5,"snapshots.list",json!({"typo":1})));w.run(8.0);
        assert!(w.take_out().iter().any(|o|matches!(o,Out::Message(m) if m["seq"]==5 && m["error"]["code"]=="request.unknown_field")));
    }).unwrap().join().unwrap();
}
