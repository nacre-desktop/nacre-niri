use super::*;

fn enabled_layout() -> Layout<TestWindow> {
    Layout::with_options(Clock::with_time(Duration::ZERO), Options::default())
}

fn shown(layout: &Layout<TestWindow>, output: &str) -> u64 {
    layout.collections().iter().find(|c| c.output.as_deref() == Some(output)).unwrap().id
}

fn add_window(layout: &mut Layout<TestWindow>, id: usize) {
    Op::AddWindow { params: TestWindowParams::new(id) }.apply(layout);
}

#[test]
fn unplug_keeps_collection_detached() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    let laptop = shown(&layout, "output1");
    Op::AddOutput(2).apply(&mut layout);
    Op::FocusOutput(2).apply(&mut layout);
    add_window(&mut layout, 1);
    let external = shown(&layout, "output2");
    let workspace_ids = layout.collections().into_iter().find(|c| c.id == external).unwrap().workspace_ids;
    Op::RemoveOutput(2).apply(&mut layout);
    let collections = layout.collections();
    let detached = collections.iter().find(|c| c.id == external).unwrap();
    assert!(detached.output.is_none());
    assert_eq!(detached.workspace_ids, workspace_ids);
    let laptop = collections.iter().find(|c| c.id == laptop).unwrap();
    assert!(laptop.workspace_ids.iter().all(|id| !workspace_ids.contains(id)));
    assert!(layout.has_window(&1));
    layout.verify_invariants();
}

#[test]
fn replug_restores_collection_and_laptop_previous() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    let laptop = shown(&layout, "output1");
    Op::AddOutput(2).apply(&mut layout);
    Op::FocusOutput(2).apply(&mut layout);
    add_window(&mut layout, 2);
    let external = shown(&layout, "output2");
    Op::RemoveOutput(2).apply(&mut layout);
    layout.focus_collection(&WorkspaceReference::Id(external));
    assert_eq!(shown(&layout, "output1"), external);
    Op::AddOutput(2).apply(&mut layout);
    assert_eq!(shown(&layout, "output2"), external);
    assert_eq!(shown(&layout, "output1"), laptop);
    layout.verify_invariants();
}

#[test]
fn emptied_detached_collection_is_removed() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    Op::AddOutput(2).apply(&mut layout);
    Op::FocusOutput(2).apply(&mut layout);
    add_window(&mut layout, 1);
    let external = shown(&layout, "output2");
    Op::RemoveOutput(2).apply(&mut layout);
    Op::CloseWindow(1).apply(&mut layout);
    layout.refresh(true);
    assert!(!layout.has_window(&1));
    assert!(!layout.collections().iter().any(|c| c.id == external));
    layout.verify_invariants();
}

#[test]
fn laptop_virtual_collection_rotates() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    let first = shown(&layout, "output1");
    layout.create_collection(Some("desk".to_owned()));
    let second = shown(&layout, "output1");
    assert_ne!(first, second);
    add_window(&mut layout, 2);
    layout.focus_collection_previous();
    assert_eq!(shown(&layout, "output1"), first);
    layout.focus_collection_next();
    assert_eq!(shown(&layout, "output1"), second);
    layout.verify_invariants();
}

#[test]
fn move_window_between_collections_preserves_window() {
    let mut layout = enabled_layout();
    Op::AddOutput(1).apply(&mut layout);
    add_window(&mut layout, 1);
    let first = shown(&layout, "output1");
    layout.create_collection(Some("desk".to_owned()));
    add_window(&mut layout, 2);
    layout.move_window_to_collection(&WorkspaceReference::Id(first));
    assert!(layout.has_window(&2));
    layout.focus_collection(&WorkspaceReference::Id(first));
    assert!(layout.active_workspace().unwrap().has_window(&2));
    layout.verify_invariants();
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]
    #[test]
    fn collection_operations_preserve_invariants(ops in prop::collection::vec(any::<Op>(), 0..100)) {
        let mut layout = enabled_layout();
        check_ops_on_layout(&mut layout, ops);
    }
}
