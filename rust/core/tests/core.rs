//! Behaviour tests for the board core. The recognizer cases are the same
//! ones the Flutter app tests (`app/test/ink_recognition_test.dart`).

use canvas_core::document::Document;
use canvas_core::editor::{Contact, Editor, PenMode, EV_DOC, EV_INK, EV_TEXT};
use canvas_core::geom::{IRect, Pt, Rect};
use canvas_core::recognize::recognize_shape;
use canvas_core::shape::{Shape, Tool};
use canvas_core::store;
use std::f64::consts::PI;
use std::sync::Arc;

fn p(x: f64, y: f64) -> Pt {
    Pt::new(x, y)
}

fn ink(points: Vec<Pt>) -> Shape {
    Shape::new("ink", Tool::Pen, points).styled(0xffc03030, 6.0, false)
}

fn segment(a: Pt, b: Pt) -> Vec<Pt> {
    (0..=20).map(|i| a + (b - a) * (i as f64 / 20.0)).collect()
}

fn pen(id: &str, points: Vec<Pt>) -> Shape {
    Shape::new(id, Tool::Pen, points)
}

// ------------------------------------------------------------- recognizer

#[test]
fn recognizes_a_closed_rectangle_keeping_identity_and_style() {
    let stroke = ink([
        segment(p(20., 30.), p(160., 30.)),
        segment(p(160., 30.), p(160., 110.)),
        segment(p(160., 110.), p(20., 110.)),
        segment(p(20., 110.), p(20., 30.)),
    ]
    .concat());
    let shape = recognize_shape(&stroke).unwrap();
    assert_eq!(shape.tool, Tool::Rectangle);
    assert_eq!(shape.id, stroke.id);
    assert_eq!(shape.color, stroke.color);
    assert_eq!(shape.width, stroke.width);
    assert_eq!(shape.points, vec![p(20., 30.), p(160., 110.)]);
}

#[test]
fn recognizes_a_circle_as_an_ellipse() {
    let stroke = ink((0..=100)
        .map(|i| {
            let a = i as f64 * PI * 2.0 / 100.0;
            p(100.0 + a.cos() * 60.0, 120.0 + a.sin() * 60.0)
        })
        .collect());
    assert_eq!(recognize_shape(&stroke).unwrap().tool, Tool::Ellipse);
}

#[test]
fn straightens_a_line_without_an_arrowhead() {
    let stroke = ink((0..80).map(|i| p(i as f64 * 3.0, i as f64 * 1.5 + (i as f64).sin() * 0.4)).collect());
    let shape = recognize_shape(&stroke).unwrap();
    assert_eq!(shape.tool, Tool::Pen);
    assert_eq!(shape.points.len(), 2);
    assert_eq!(shape.points[0], stroke.points[0]);
    assert_eq!(shape.points[1], *stroke.points.last().unwrap());
}

#[test]
fn recognizes_a_clearly_traced_arrow() {
    let stroke = ink([
        segment(p(0., 0.), p(200., 0.)),
        segment(p(200., 0.), p(150., -35.)),
        segment(p(150., -35.), p(200., 0.)),
        segment(p(200., 0.), p(150., 35.)),
    ]
    .concat());
    let shape = recognize_shape(&stroke).unwrap();
    assert_eq!(shape.tool, Tool::Arrow);
    assert_eq!(shape.points, vec![p(0., 0.), p(200., 0.)]);
}

#[test]
fn keeps_scribbles_tiny_marks_open_curves_and_shapes_unchanged() {
    let scribble =
        ink(vec![p(0., 0.), p(70., 100.), p(120., 10.), p(15., 80.), p(150., 90.), p(30., 20.), p(120., 120.)]);
    assert!(recognize_shape(&scribble).is_none());
    assert!(recognize_shape(&ink(vec![p(0., 0.), p(3., 2.)])).is_none());
    let arc = ink((0..40).map(|i| p(i as f64 * 3.0, (i as f64 / 39.0 * PI).sin() * 80.0)).collect());
    assert!(recognize_shape(&arc).is_none());
    let boxed = Shape::new("box", Tool::Rectangle, vec![p(0., 0.), p(100., 100.)]);
    assert!(recognize_shape(&boxed).is_none());
}

// ------------------------------------------------------- document and JSON

#[test]
fn reads_a_board_written_by_the_flutter_app_and_writes_it_back() {
    // Exactly what Dart's jsonEncode produces: doubles keep ".0", colour is an
    // integer, fontSize is omitted when it is the default 24.
    let source = r#"{"version":1,"id":"b1","title":"Maths","updated":"2026-11-16T09:30:00.123456","shapes":[{"id":"a-1","tool":"pen","points":[[10.0,20.5],[30.25,-4.0]],"color":4280098077,"width":3.5,"filled":false,"text":""},{"id":"a-2","tool":"rectangle","points":[[0.0,0.0],[100.0,50.0]],"color":4292882737,"width":2.0,"filled":true,"text":""},{"id":"a-3","tool":"text","points":[[5.0,6.0]],"color":4280098077,"width":3.0,"filled":false,"text":"नमस्ते\nhi","fontSize":32.0}],"share":{"server":"https://example.test","room":"r","token":"t"},"synced":[]}"#;
    let board = store::parse(source).unwrap();
    assert_eq!((board.id.as_str(), board.title.as_str()), ("b1", "Maths"));
    assert_eq!(board.shapes.len(), 3);
    assert_eq!(board.shapes[0].points[1], p(30.25, -4.0));
    assert_eq!(board.shapes[1].color, 0xffe03131);
    assert!(board.shapes[1].filled);
    assert_eq!(board.shapes[2].font_size, 32.0);
    assert_eq!(board.shapes[2].text, "नमस्ते\nhi");
    let shapes: Vec<Arc<Shape>> = board.shapes.into_iter().map(Arc::new).collect();
    let written = store::encode(&board.id, &board.title, &board.updated, &shapes, &board.extra);
    let a: serde_json::Value = serde_json::from_str(source).unwrap();
    let b: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(a, b, "sharing state and every shape field survive a round trip");
    // Dart reads numbers with `as num`, so the text form of doubles matters
    // less than the values, but whole doubles must stay doubles.
    assert!(written.contains(r#""points":[[10.0,20.5],[30.25,-4.0]]"#));
    assert!(written.contains(r#""color":4280098077"#));
    assert!(!written.contains(r#""fontSize":24"#));
}

#[test]
fn rejects_bad_boards() {
    assert!(Document::decode(r#"{"version":2,"shapes":[]}"#).is_err());
    let dup = r#"{"version":1,"shapes":[{"id":"x","tool":"pen","points":[[0,0]]},{"id":"x","tool":"pen","points":[[1,1]]}]}"#;
    assert!(Document::decode(dup).is_err());
    let width = r#"{"version":1,"shapes":[{"id":"x","tool":"pen","points":[[0,0]],"width":0}]}"#;
    assert!(Document::decode(width).is_err());
    let tool = r#"{"version":1,"shapes":[{"id":"x","tool":"laser","points":[[0,0]]}]}"#;
    assert!(Document::decode(tool).is_err());
    // Defaults apply when optional keys are missing.
    let minimal = Document::decode(r#"{"version":1,"shapes":[{"id":"x","tool":"pen","points":[[0,0]]}]}"#).unwrap();
    assert_eq!((minimal[0].color, minimal[0].width, minimal[0].font_size), (0xff1d1d1d, 3.0, 24.0));
}

#[test]
fn undo_and_redo_restore_shapes_to_their_layer() {
    let mut doc = Document::new();
    for id in ["a", "b", "c"] {
        doc.add(pen(id, vec![p(0., 0.), p(10., 10.)]));
    }
    let order = |d: &Document| d.shapes().iter().map(|s| s.id.clone()).collect::<Vec<_>>().join("");
    doc.remove_many(vec!["a".to_string(), "b".to_string()]);
    assert_eq!(order(&doc), "c");
    doc.undo();
    assert_eq!(order(&doc), "abc", "removed shapes come back underneath, in order");
    doc.redo();
    assert_eq!(order(&doc), "c");
    doc.undo();
    doc.replace(pen("b", vec![p(5., 5.), p(6., 6.)]));
    assert!(!doc.can_redo(), "a new edit clears redo");
    assert_eq!(doc.get("b").unwrap().points[0], p(5., 5.));
    doc.undo();
    assert_eq!(doc.get("b").unwrap().points[0], p(0., 0.));
    for _ in 0..10 {
        doc.undo();
    }
    assert_eq!(doc.shapes().len(), 0);
    assert!(!doc.can_undo());
    // Swap is one undo step.
    doc.add(pen("ink", vec![p(0., 0.), p(1., 1.)]));
    doc.swap(vec!["ink".into()], vec![Shape::new("t", Tool::Text, vec![p(0., 0.)]).with_text("hi", 24.0)]);
    assert_eq!(order(&doc), "t");
    doc.undo();
    assert_eq!(order(&doc), "ink");
}

#[test]
fn history_is_capped_and_selection_is_pruned() {
    let mut doc = Document::new();
    for i in 0..130 {
        doc.add(pen(&format!("s{i}"), vec![p(0., 0.), p(1., 1.)]));
    }
    let mut undone = 0;
    while doc.can_undo() {
        doc.undo();
        undone += 1;
    }
    assert_eq!(undone, 100);
    assert_eq!(doc.shapes().len(), 30);
    doc.select(Some("s3"));
    assert_eq!(doc.selected_ids(), ["s3".to_string()]);
    doc.remove_many(vec!["s3".to_string()]);
    assert!(doc.selected_ids().is_empty());
}

#[test]
fn hit_testing_matches_each_shape_kind() {
    let rect = Shape::new("r", Tool::Rectangle, vec![p(0., 0.), p(100., 60.)]);
    assert!(rect.hits(p(50., 2.), 10.0), "on the outline");
    assert!(!rect.hits(p(50., 30.), 10.0), "inside an unfilled box");
    assert!(rect.clone().styled(0xff000000, 3.0, true).hits(p(50., 30.), 10.0), "inside a filled box");
    let ellipse = Shape::new("e", Tool::Ellipse, vec![p(0., 0.), p(100., 100.)]);
    assert!(ellipse.hits(p(100., 50.), 5.0));
    assert!(!ellipse.hits(p(50., 50.), 5.0));
    assert!(!ellipse.hits(p(2., 2.), 5.0), "the corner of the bounding box is not the ellipse");
    let line = pen("l", vec![p(0., 0.), p(100., 0.)]);
    assert!(line.hits(p(50., 8.), 10.0));
    assert!(!line.hits(p(50., 20.), 10.0));
    let dot = pen("d", vec![p(5., 5.)]);
    assert!(dot.hits(p(8., 8.), 10.0));
    let text = Shape::new("t", Tool::Text, vec![p(10., 10.)]).with_text("hello", 24.0);
    assert!(text.hits(p(20., 20.), 0.0));
    let mut doc = Document::new();
    doc.add(line);
    doc.add(pen("top", vec![p(0., 0.), p(100., 0.)]));
    assert_eq!(doc.hit_test(p(50., 1.), 10.0).unwrap().id, "top", "topmost shape wins");
}

#[test]
fn board_files_save_atomically_and_list_newest_first() {
    let dir = std::env::temp_dir().join(format!("canvas-core-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let shapes = vec![Arc::new(pen("a", vec![p(1., 2.), p(3., 4.)]))];
    let extra = serde_json::Map::new();
    store::save_atomic(&dir, "one", &store::encode("one", "First", "2026-01-01T10:00:00.000", &shapes, &extra)).unwrap();
    store::save_atomic(&dir, "two", &store::encode("two", "Second", "2026-02-01T10:00:00.000", &[], &extra)).unwrap();
    assert!(!dir.join("one.tmp").exists(), "no temporary file is left behind");
    let list = store::list(&dir);
    assert_eq!(list.iter().map(|b| b.title.as_str()).collect::<Vec<_>>(), ["Second", "First"]);
    assert_eq!(list[1].shapes, 1);
    store::rename(&dir, "one", "Renamed", "2026-03-01T10:00:00.000").unwrap();
    let board = store::load(&dir, "one").unwrap();
    assert_eq!(board.title, "Renamed");
    assert_eq!(board.shapes[0].points[1], p(3., 4.));
    assert_eq!(store::list(&dir)[0].id, "one");
    assert!(store::save_atomic(&dir, "../evil", "{}").is_err(), "ids cannot escape the folder");
    store::delete(&dir, "one").unwrap();
    store::delete(&dir, "one").unwrap();
    assert_eq!(store::list(&dir).len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ------------------------------------------------------------------ editor

const W: u32 = 400;
const H: u32 = 300;

fn editor() -> Editor {
    let mut e = Editor::new();
    e.resize(W, H, 1.0);
    e.set_grid(false);
    e.set_auto_convert(false);
    e.set_auto_text(false);
    e.render();
    e
}

/// An editor as the app starts it: shapes and handwriting convert by themselves.
fn auto_editor() -> Editor {
    let mut e = editor();
    e.set_auto_convert(true);
    e.set_auto_text(true);
    e
}

#[test]
fn handwriting_can_be_left_as_ink_while_shapes_still_tidy() {
    let mut e = auto_editor();
    e.set_auto_text(false);
    drag(&mut e, &letter(50., 100.), 0);
    assert!(e.next_deadline().is_none());
    assert!(e.take_ink_request().is_none());
    assert_eq!(e.doc.shapes()[0].tool, Tool::Pen);
    drag(&mut e, &line(p(60., 200.), p(260., 204.), 20), 3000);
    assert_eq!(e.doc.shapes()[1].points.len(), 2, "a straight stroke still becomes a line");
}

fn pixel(e: &Editor, x: u32, y: u32) -> [u8; 4] {
    let f = e.frame();
    let i = (y as usize * f.w as usize + x as usize) * 4;
    [f.data[i], f.data[i + 1], f.data[i + 2], f.data[i + 3]]
}

fn is_ink(px: [u8; 4]) -> bool {
    px[0] < 90 && px[1] < 90 && px[2] < 90
}

/// Drags pointer 0 through `points`, rendering after every sample as the
/// render thread would, and returns the largest rectangle repainted.
fn drag(e: &mut Editor, points: &[Pt], start_time: u64) -> IRect {
    let mut worst = IRect::new(0, 0, 0, 0);
    let mut note = |r: Option<IRect>| {
        if let Some(r) = r {
            if r.width() * r.height() > worst.width() * worst.height() {
                worst = r;
            }
        }
    };
    e.pointer_down(0, points[0], start_time);
    note(e.render());
    for (i, pt) in points.iter().enumerate().skip(1) {
        e.pointer_move(0, *pt, start_time + i as u64 * 8);
        note(e.render());
    }
    e.pointer_up(0, *points.last().unwrap(), start_time + points.len() as u64 * 8, false);
    note(e.render());
    worst
}

fn line(a: Pt, b: Pt, n: usize) -> Vec<Pt> {
    (0..=n).map(|i| a + (b - a) * (i as f64 / n as f64)).collect()
}

/// The frame must equal what a from-scratch repaint produces.
fn assert_matches_full_repaint(e: &mut Editor) {
    let incremental = e.frame().data.clone();
    e.invalidate_all();
    e.render();
    let fresh = &e.frame().data;
    let differing = incremental.iter().zip(fresh.iter()).filter(|(a, b)| a.abs_diff(**b) > 2).count();
    assert_eq!(differing, 0, "incrementally painted frame differs from a full repaint");
}

#[test]
fn drawing_a_stroke_puts_ink_on_screen_immediately_and_commits_it() {
    let mut e = editor();
    assert!(!is_ink(pixel(&e, 100, 100)));
    e.pointer_down(0, p(50., 100.), 0);
    e.render();
    assert!(is_ink(pixel(&e, 50, 100)), "the first touch already shows a dot");
    for (i, pt) in line(p(50., 100.), p(300., 100.), 25).iter().enumerate().skip(1) {
        e.pointer_move(0, *pt, i as u64 * 8);
        let damaged = e.render().expect("each sample repaints something");
        assert!(damaged.width() < 60 && damaged.height() < 40, "only the new piece is repainted: {damaged:?}");
        assert!(is_ink(pixel(&e, pt.x as u32 - 2, 100)), "ink follows the finger with no lag");
    }
    assert_eq!(e.doc.shapes().len(), 0, "not committed until the finger lifts");
    e.pointer_up(0, p(300., 100.), 300, false);
    e.render();
    assert_eq!(e.doc.shapes().len(), 1);
    let s = &e.doc.shapes()[0];
    assert_eq!((s.tool, s.color, s.width), (Tool::Pen, 0xff1d1d1d, 3.5));
    assert!(is_ink(pixel(&e, 175, 100)));
    assert_ne!(e.take_events() & EV_DOC, 0, "the host is told to save");
    assert_matches_full_repaint(&mut e);
}

#[test]
fn a_new_stroke_never_repaints_the_existing_board() {
    let mut e = editor();
    let many: Vec<Shape> = (0..600)
        .map(|i| {
            let y = 10.0 + (i % 40) as f64 * 7.0;
            let x = 10.0 + (i / 40) as f64 * 25.0;
            pen(&format!("s{i}"), vec![p(x, y), p(x + 20.0, y + 3.0), p(x + 22.0, y - 2.0)])
        })
        .collect();
    e.load(many).unwrap();
    e.render();
    let before = e.full_repaints;
    let wave: Vec<Pt> = (0..80).map(|i| p(20.0 + i as f64 * 4.0, 150.0 + (i as f64 / 6.0).sin() * 40.0)).collect();
    let worst = drag(&mut e, &wave, 0);
    assert_eq!(e.full_repaints, before, "the cached scene is reused for every sample and for the commit");
    assert!(
        (worst.width() * worst.height()) < (W * H / 2) as i32,
        "largest repaint {worst:?} should be the stroke's own bounds, not the screen"
    );
    assert_eq!(e.doc.shapes().len(), 601);
    assert_matches_full_repaint(&mut e);
}

#[test]
fn live_ink_looks_the_same_as_the_committed_stroke() {
    let mut e = editor();
    let curl: Vec<Pt> = (0..120)
        .map(|i| {
            let a = i as f64 / 12.0;
            p(200.0 + a.cos() * (20.0 + i as f64), 150.0 + a.sin() * (10.0 + i as f64 * 0.8))
        })
        .collect();
    e.pointer_down(0, curl[0], 0);
    for (i, pt) in curl.iter().enumerate().skip(1) {
        e.pointer_move(0, *pt, i as u64 * 8);
        e.render();
    }
    let live = e.frame().data.clone();
    e.pointer_up(0, *curl.last().unwrap(), 2000, false);
    e.render();
    let committed = &e.frame().data;
    let differing = live.iter().zip(committed.iter()).filter(|(a, b)| a.abs_diff(**b) > 24).count();
    assert!(differing < 40, "{differing} bytes changed when the stroke was committed");
}

#[test]
fn smart_shape_pen_replaces_a_rough_box_with_a_rectangle() {
    let mut e = editor();
    e.set_tool(Tool::Pen, PenMode::Shape);
    let rough = [
        line(p(60., 60.), p(260., 62.), 20),
        line(p(260., 62.), p(258., 200.), 20),
        line(p(258., 200.), p(62., 198.), 20),
        line(p(62., 198.), p(60., 64.), 20),
    ]
    .concat();
    drag(&mut e, &rough, 0);
    assert_eq!(e.doc.shapes().len(), 1);
    assert_eq!(e.doc.shapes()[0].tool, Tool::Rectangle);
    assert_matches_full_repaint(&mut e);
    // In ink mode the same gesture stays ink.
    e.set_tool(Tool::Pen, PenMode::Ink);
    drag(&mut e, &rough, 5000);
    assert_eq!(e.doc.shapes()[1].tool, Tool::Pen);
}

#[test]
fn holding_still_snaps_the_stroke_to_a_shape() {
    let mut e = editor();
    let pts = line(p(40., 40.), p(300., 180.), 30);
    e.pointer_down(0, pts[0], 0);
    for (i, pt) in pts.iter().enumerate().skip(1) {
        e.pointer_move(0, *pt, i as u64 * 8);
    }
    let deadline = e.next_deadline().expect("a hold timer is running");
    e.tick(deadline - 1);
    e.render();
    e.tick(deadline);
    e.render();
    assert!(e.next_deadline().is_none());
    e.pointer_up(0, *pts.last().unwrap(), deadline + 50, false);
    e.render();
    let s = &e.doc.shapes()[0];
    assert_eq!((s.tool, s.points.len()), (Tool::Pen, 2), "snapped to a straight line");
    assert_matches_full_repaint(&mut e);
}

#[test]
fn shape_tools_create_shapes_and_return_to_select() {
    let mut e = editor();
    e.apply_style(Some(0xffe03131), Some(5.0), Some(true));
    for (tool, y) in [(Tool::Rectangle, 20.), (Tool::Ellipse, 110.), (Tool::Arrow, 200.)] {
        e.set_tool(tool, PenMode::Ink);
        drag(&mut e, &line(p(30., y), p(200., y + 70.), 10), 0);
        assert_eq!(e.tool(), Tool::Select);
        let s = e.doc.shapes().last().unwrap().clone();
        assert_eq!((s.tool, s.color, s.width, s.filled), (tool, 0xffe03131, 5.0, true));
        assert_eq!(s.points, vec![p(30., y), p(200., y + 70.)]);
        assert_eq!(e.doc.selected_ids(), [s.id.clone()]);
    }
    assert_matches_full_repaint(&mut e);
    // A tap with a shape tool makes nothing.
    e.set_tool(Tool::Rectangle, PenMode::Ink);
    drag(&mut e, &[p(300., 20.), p(301., 21.)], 0);
    assert_eq!(e.doc.shapes().len(), 3);
}

#[test]
fn eraser_removes_what_it_touches_in_one_undo_step() {
    let mut e = editor();
    drag(&mut e, &line(p(50., 50.), p(350., 50.), 20), 0);
    drag(&mut e, &line(p(50., 120.), p(350., 120.), 20), 1000);
    drag(&mut e, &line(p(50., 250.), p(350., 250.), 20), 2000);
    e.set_tool(Tool::Eraser, PenMode::Ink);
    e.pointer_down(0, p(200., 20.), 3000);
    e.pointer_move(0, p(200., 150.), 3010);
    e.render();
    assert!(!is_ink(pixel(&e, 100, 50)), "erased ink fades while the finger is still down");
    assert_eq!(e.doc.shapes().len(), 3);
    e.pointer_up(0, p(200., 150.), 3020, false);
    e.render();
    assert_eq!(e.doc.shapes().len(), 1);
    assert!(is_ink(pixel(&e, 100, 250)));
    assert_matches_full_repaint(&mut e);
    e.undo();
    e.render();
    assert_eq!(e.doc.shapes().len(), 3);
    assert!(is_ink(pixel(&e, 100, 50)));
    assert_matches_full_repaint(&mut e);
}

#[test]
fn select_moves_resizes_and_marquee_selects() {
    let mut e = editor();
    e.set_tool(Tool::Rectangle, PenMode::Ink);
    drag(&mut e, &line(p(50., 50.), p(150., 110.), 6), 0);
    let id = e.doc.shapes()[0].id.clone();
    // Drag by its top edge.
    drag(&mut e, &line(p(100., 50.), p(160., 90.), 12), 1000);
    assert_eq!(e.doc.shapes()[0].points, vec![p(110., 90.), p(210., 150.)]);
    assert_matches_full_repaint(&mut e);
    // Drag the bottom-right handle (frame is inflated by 6) to double the size.
    drag(&mut e, &line(p(216., 156.), p(322., 222.), 12), 2000);
    let b = e.doc.shapes()[0].bounds();
    // Scaling is about the opposite corner of the selection frame, as in the Flutter app.
    let (sx, sy) = ((322.0 - 104.0) / (216.0 - 104.0), (222.0 - 84.0) / (156.0 - 84.0));
    assert!((b.width() - 100.0 * sx).abs() < 0.01 && (b.height() - 60.0 * sy).abs() < 0.01, "{b:?}");
    assert!(b.width() > 190.0 && b.height() > 110.0);
    assert_matches_full_repaint(&mut e);
    // Tap empty space clears the selection; a marquee selects again.
    drag(&mut e, &[p(20., 280.), p(20., 280.)], 3000);
    assert!(e.doc.selected_ids().is_empty());
    drag(&mut e, &line(p(5., 5.), p(390., 290.), 8), 4000);
    assert_eq!(e.doc.selected_ids(), [id]);
    e.delete_selection();
    e.render();
    assert_eq!(e.doc.shapes().len(), 0);
    e.undo();
    e.undo();
    assert_eq!(e.doc.shapes()[0].points, vec![p(110., 90.), p(210., 150.)], "undo steps back through the resize");
}

#[test]
fn hand_tool_pans_and_the_scrolled_scene_matches_a_repaint() {
    let mut e = editor();
    e.set_grid(true);
    drag(&mut e, &line(p(50., 50.), p(350., 250.), 20), 0);
    e.set_tool(Tool::Hand, PenMode::Ink);
    let before = e.full_repaints;
    drag(&mut e, &line(p(200., 150.), p(140., 190.), 10), 1000);
    assert_eq!(e.camera(), p(-60., 40.));
    assert_eq!(e.full_repaints, before, "panning slides the cached scene");
    assert_matches_full_repaint(&mut e);
    assert_eq!(e.doc.shapes().len(), 1);
}

#[test]
fn two_fingers_pinch_to_zoom_about_their_midpoint() {
    let mut e = editor();
    e.pointer_down(0, p(150., 150.), 0);
    e.pointer_down(1, p(250., 150.), 30);
    e.pointer_move(0, p(100., 150.), 40);
    e.pointer_move(1, p(300., 150.), 40);
    assert!((e.zoom() - 2.0).abs() < 1e-9);
    assert_eq!(e.world(p(200., 150.)), p(200., 150.), "the midpoint stays put");
    e.pointer_up(0, p(100., 150.), 50, false);
    e.pointer_move(1, p(390., 290.), 60);
    e.pointer_up(1, p(390., 290.), 70, false);
    assert_eq!(e.doc.shapes().len(), 0, "the pinch never leaves ink behind");
    e.render();
    assert_matches_full_repaint(&mut e);
}

#[test]
fn two_finger_pan_with_spacing_jitter_scrolls_without_full_repaints() {
    let mut e = editor();
    e.set_grid(true);
    drag(&mut e, &line(p(50., 50.), p(350., 250.), 20), 0);
    e.render();
    let before = e.full_repaints;
    e.pointer_down(0, p(100., 150.), 1000);
    e.pointer_down(1, p(220., 150.), 1020);
    // Natural two-finger pan: fingers drift 120 px right with ±6 px spacing wobble.
    for i in 1..=12u64 {
        let shift = i as f64 * 10.0;
        let wobble = if i % 2 == 0 { 6.0 } else { -5.0 };
        e.pointer_move(0, p(100. + shift, 150. + shift * 0.3), 1020 + i * 16);
        e.pointer_move(1, p(220. + shift + wobble, 150. + shift * 0.3), 1020 + i * 16);
        assert!(e.render().is_some());
    }
    assert_eq!(e.zoom(), 1.0, "small spacing jitter stays locked to a pure pan");
    assert_eq!(e.full_repaints, before, "two-finger pan slides the cached scene without full repaints");
    e.pointer_up(0, p(220., 186.), 1300, false);
    e.pointer_up(1, p(346., 186.), 1310, false);
    e.render();
    assert_matches_full_repaint(&mut e);
}

#[test]
fn active_pinch_zoom_scales_snapshot_and_repaints_crisply_on_lift() {
    let mut e = editor();
    drag(&mut e, &line(p(50., 50.), p(350., 250.), 20), 0);
    e.render();
    let before = e.full_repaints;
    e.pointer_down(0, p(150., 150.), 1000);
    e.pointer_down(1, p(250., 150.), 1020);
    for i in 1..=8u64 {
        let d = i as f64 * 10.0;
        e.pointer_move(0, p(150. - d, 150.), 1020 + i * 16);
        e.pointer_move(1, p(250. + d, 150.), 1020 + i * 16);
        assert!(e.render().is_some());
    }
    assert_eq!(e.full_repaints, before, "active pinch-zoom scales the cached snapshot");
    e.pointer_up(0, p(70., 150.), 1200, false);
    e.pointer_up(1, p(330., 150.), 1210, false);
    assert!(e.render().is_some());
    assert_eq!(e.full_repaints, before + 1, "one crisp full vector repaint after lift");
    assert_matches_full_repaint(&mut e);
}

#[test]
fn a_late_second_touch_does_not_interrupt_a_stroke() {
    let mut e = editor();
    e.pointer_down(0, p(50., 50.), 0);
    for i in 1..=30 {
        e.pointer_move(0, p(50.0 + i as f64 * 5.0, 50.), i * 10);
    }
    e.pointer_down(1, p(300., 250.), 400); // a resting palm
    e.pointer_move(1, p(305., 255.), 410);
    e.pointer_move(0, p(250., 60.), 420);
    e.pointer_up(1, p(305., 255.), 430, false);
    e.pointer_up(0, p(250., 60.), 440, false);
    assert_eq!(e.doc.shapes().len(), 1);
    assert_eq!(e.zoom(), 1.0);
    assert_eq!(*e.doc.shapes()[0].points.last().unwrap(), p(250., 60.));
}

#[test]
fn text_tool_asks_the_host_and_commits_text() {
    let mut e = editor();
    e.set_tool(Tool::Text, PenMode::Ink);
    drag(&mut e, &[p(120., 80.), p(121., 80.)], 0);
    assert_ne!(e.take_events() & EV_TEXT, 0);
    let request = e.take_text_request().unwrap();
    assert_eq!((request.id.as_str(), request.x), ("", 121.0));
    e.commit_text("", request.x, request.y, "Hello");
    let s = e.doc.shapes()[0].clone();
    assert_eq!((s.tool, s.text.as_str(), s.font_size), (Tool::Text, "Hello", 28.0));
    assert_eq!(e.tool(), Tool::Select);
    e.commit_text(&s.id, 0.0, 0.0, "Changed");
    assert_eq!(e.doc.shapes()[0].text, "Changed");
    assert_eq!(e.doc.shapes()[0].points, s.points);
    // Double tap on text asks to edit it.
    let inside = p(170., 98.); // the middle, clear of the resize handles
    drag(&mut e, &[inside, inside], 1000);
    drag(&mut e, &[inside, inside], 1200);
    assert_eq!(e.take_text_request().unwrap().text, "Changed");
}

#[test]
fn a_cancelled_touch_leaves_nothing_behind() {
    let mut e = editor();
    e.pointer_down(0, p(50., 50.), 0);
    e.pointer_move(0, p(200., 200.), 10);
    e.render();
    assert!(is_ink(pixel(&e, 125, 125)));
    e.pointer_up(0, p(200., 200.), 20, true);
    e.render();
    assert_eq!(e.doc.shapes().len(), 0);
    assert!(!is_ink(pixel(&e, 125, 125)));
    assert_matches_full_repaint(&mut e);
}

#[test]
fn high_density_screens_keep_board_units() {
    let mut e = Editor::new();
    e.resize(800, 600, 2.0);
    e.set_grid(false);
    e.render();
    // Touches arrive in pixels; the board stores logical units.
    drag(&mut e, &line(p(100., 100.), p(500., 100.), 20), 0);
    let s = &e.doc.shapes()[0];
    assert_eq!((s.points[0], *s.points.last().unwrap()), (p(50., 50.), p(250., 50.)));
    assert!(is_ink(pixel(&e, 300, 100)));
    assert!(is_ink(pixel(&e, 300, 102)), "a 3.5 unit stroke is 7 pixels wide at 2x");
    assert!(!is_ink(pixel(&e, 300, 105)));
    e.fit();
    e.render();
    assert_matches_full_repaint(&mut e);
}

#[test]
fn thumbnails_render() {
    let shapes = vec![Arc::new(pen("a", vec![p(0., 0.), p(500., 500.)]))];
    let px = canvas_core::render::thumbnail(&shapes, 64, 64);
    assert_eq!(px.len(), 64 * 64 * 4);
    let mid = (32 * 64 + 32) * 4;
    assert!(px[mid] < 235, "the diagonal passes through the middle: {}", px[mid]);
    assert_eq!(&px[(5 * 64 + 58) * 4..(5 * 64 + 58) * 4 + 3], &[0xf9, 0xfa, 0xfb], "corners are background");
    let _ = Rect::ZERO;
}

fn rough_box(l: f64, t: f64, r: f64, b: f64) -> Vec<Pt> {
    [
        line(p(l, t), p(r, t + 2.), 20),
        line(p(r, t + 2.), p(r - 2., b), 20),
        line(p(r - 2., b), p(l + 2., b - 2.), 20),
        line(p(l + 2., b - 2.), p(l, t + 4.), 20),
    ]
    .concat()
}

/// A zigzag the size of a handwritten letter.
fn letter(x: f64, y: f64) -> Vec<Pt> {
    [line(p(x, y + 40.), p(x + 10., y), 6), line(p(x + 10., y), p(x + 20., y + 40.), 6), line(p(x + 20., y + 40.), p(x + 30., y), 6)]
        .concat()
}

#[test]
fn shapes_are_tidied_automatically_when_the_pen_lifts() {
    let mut e = auto_editor();
    drag(&mut e, &rough_box(60., 60., 260., 200.), 0);
    assert_eq!(e.doc.shapes()[0].tool, Tool::Rectangle);
    assert!(e.take_ink_request().is_none());
    assert_matches_full_repaint(&mut e);
    // A box the size of a letter stays ink: it is probably writing.
    let mut e = auto_editor();
    drag(&mut e, &rough_box(60., 60., 100., 100.), 0);
    assert_eq!(e.doc.shapes()[0].tool, Tool::Pen);
}

#[test]
fn handwriting_becomes_typed_text_after_a_pause() {
    let mut e = auto_editor();
    e.take_events();
    drag(&mut e, &letter(50., 100.), 0);
    let first_deadline = e.next_deadline().expect("waiting for more writing");
    // The next letter arrives before the pause ends and restarts the wait.
    drag(&mut e, &letter(95., 100.), 600);
    e.tick(first_deadline);
    assert!(e.take_ink_request().is_none());
    let deadline = e.next_deadline().unwrap();
    assert!(deadline > first_deadline);
    e.tick(deadline);
    assert_ne!(e.take_events() & EV_INK, 0);
    let request = e.take_ink_request().expect("both letters are handed over together");
    assert_eq!(request.strokes.len(), 2);
    assert!(e.next_deadline().is_none());
    e.commit_ink(&request.ids, " hi ");
    e.render();
    assert_eq!(e.doc.shapes().len(), 1);
    let typed = e.doc.shapes()[0].clone();
    assert_eq!((typed.tool, typed.text.as_str()), (Tool::Text, "hi"));
    assert!(typed.bounds().overlaps(&request.bounds));
    assert_matches_full_repaint(&mut e);
    // One undo brings the ink back, and it is not converted a second time.
    e.undo();
    assert_eq!(e.doc.shapes().len(), 2);
    assert!(e.doc.shapes().iter().all(|s| s.tool == Tool::Pen));
    assert!(e.next_deadline().is_none());
    // Unreadable writing is left alone.
    e.commit_ink(&request.ids, "  ");
    assert_eq!(e.doc.shapes().len(), 2);
}

#[test]
fn writing_far_away_is_read_separately_and_drawings_are_left_alone() {
    let mut e = auto_editor();
    drag(&mut e, &letter(20., 20.), 0);
    drag(&mut e, &letter(300., 230.), 400);
    let first = e.take_ink_request().expect("the first word is handed over when a new one starts");
    assert_eq!(first.strokes.len(), 1);
    // Stale answers are ignored once a stroke has gone.
    let id = first.ids[0].clone();
    e.set_tool(Tool::Select, PenMode::Ink);
    assert!(e.take_ink_request().is_some(), "changing tool reads what is waiting");
    e.doc.select(Some(&id));
    e.delete_selection();
    e.commit_ink(&first.ids, "x");
    assert!(e.doc.shapes().iter().all(|s| s.tool == Tool::Pen));
    // A scribble that covers most of the screen is a drawing.
    let mut e = auto_editor();
    let mut scribble = Vec::new();
    for i in 0..6 {
        scribble.extend(line(p(40. + i as f64 * 40., 30.), p(60. + i as f64 * 40., 270.), 12));
    }
    drag(&mut e, &scribble, 0);
    let deadline = e.next_deadline().unwrap();
    e.tick(deadline);
    assert!(e.take_ink_request().is_none());
    assert_eq!(e.doc.shapes()[0].tool, Tool::Pen);
}

#[test]
fn a_shape_followed_by_writing_goes_back_to_ink() {
    let mut e = auto_editor();
    e.resize(800, 600, 1.0);
    e.render();
    // A tall "l" on its own looks like a line...
    drag(&mut e, &line(p(60., 60.), p(62., 200.), 20), 0);
    assert_eq!(e.doc.shapes()[0].points.len(), 2);
    // ...until the rest of the word follows.
    drag(&mut e, &line(p(90., 110.), p(92., 200.), 20), 500);
    assert!(e.doc.shapes()[0].points.len() > 2, "the first stroke is ink again");
    let deadline = e.next_deadline().unwrap();
    e.tick(deadline);
    assert_eq!(e.take_ink_request().unwrap().strokes.len(), 2);
}

#[test]
fn palms_are_ignored() {
    let big = Contact { size: 90.0, stylus: false, eraser: false, norm: 0.0 };
    let finger = Contact { size: 14.0, stylus: false, eraser: false, norm: 0.0 };
    // A palm resting before and during a stroke changes nothing.
    let mut e = editor();
    e.pointer_down_with(5, p(300., 250.), 0, big);
    e.pointer_move_with(5, p(310., 255.), 5, big);
    e.pointer_down_with(0, p(50., 50.), 10, finger);
    for i in 1..=20 {
        e.pointer_move_with(0, p(50.0 + i as f64 * 5.0, 50.), 10 + i * 8, finger);
        e.pointer_move_with(5, p(310., 255. + i as f64), 10 + i * 8, big);
    }
    e.pointer_up(0, p(150., 50.), 200, false);
    e.pointer_up(5, p(310., 275.), 210, false);
    assert_eq!(e.doc.shapes().len(), 1);
    assert_eq!(e.zoom(), 1.0);
    assert!(e.doc.shapes()[0].bounds().b < 60.0);
    assert!(!e.pointer_down_now());

    // A hand that lands edge first and then flattens: its mark is dropped.
    let mut e = editor();
    e.pointer_down_with(1, p(200., 200.), 0, finger);
    e.pointer_move_with(1, p(204., 204.), 30, finger);
    e.pointer_move_with(1, p(206., 206.), 60, big);
    e.pointer_move_with(1, p(240., 240.), 90, big);
    e.pointer_up(1, p(240., 240.), 120, false);
    e.render();
    assert_eq!(e.doc.shapes().len(), 0);
    assert_matches_full_repaint(&mut e);

    // With a pen, the hand holding it never draws or zooms.
    let pen = Contact { size: 4.0, stylus: true, eraser: false, norm: 0.0 };
    let mut e = editor();
    e.pointer_down_with(7, p(250., 220.), 0, finger); // hand touches first
    e.pointer_move_with(7, p(256., 226.), 20, finger);
    e.pointer_down_with(0, p(50., 50.), 40, pen);
    e.pointer_move_with(0, p(150., 50.), 80, pen);
    e.pointer_move_with(7, p(260., 230.), 90, finger);
    e.pointer_up(0, p(150., 50.), 120, false);
    e.pointer_down_with(8, p(120., 200.), 200, finger); // just after the pen lifts
    e.pointer_move_with(8, p(160., 240.), 240, finger);
    e.pointer_up(8, p(160., 240.), 260, false);
    e.pointer_up(7, p(260., 230.), 300, false);
    assert_eq!(e.doc.shapes().len(), 1);
    assert!(e.doc.shapes()[0].bounds().b < 60.0);
    assert_eq!(e.zoom(), 1.0);

    // Switched off, a big contact draws like any other.
    let mut e = editor();
    e.set_palm_rejection(false, None);
    e.pointer_down_with(1, p(50., 50.), 0, big);
    e.pointer_move_with(1, p(150., 50.), 40, big);
    e.pointer_up(1, p(150., 50.), 80, false);
    assert_eq!(e.doc.shapes().len(), 1);
}

#[test]
fn fonts_are_saved_and_applied_to_selected_text() {
    let mut e = editor();
    e.set_font(1);
    e.commit_text("", 20.0, 20.0, "Serif");
    let id = e.doc.shapes()[0].id.clone();
    assert_eq!(e.doc.shapes()[0].font, 1);
    assert_eq!(e.selected_text_count(), 1);
    e.set_font(2);
    assert_eq!(e.doc.shapes()[0].font, 2);
    let before = e.doc.shapes()[0].font_size;
    e.scale_text(1.25);
    assert_eq!(e.doc.shapes()[0].font_size, before * 1.25);
    e.set_font(99); // unknown fonts are ignored
    assert_eq!(e.font(), 2);
    let json = e.doc.shapes()[0].to_json();
    assert_eq!(json["font"], "mono");
    let back = Shape::from_json(&json).unwrap();
    assert_eq!((back.font, back.id.as_str()), (2, id.as_str()));
    // Boards without a font, or with one this version does not know, use the default.
    let mut plain = json.clone();
    plain.as_object_mut().unwrap().remove("font");
    assert_eq!(Shape::from_json(&plain).unwrap().font, 0);
    plain["font"] = "comic".into();
    assert_eq!(Shape::from_json(&plain).unwrap().font, 0);
    e.undo();
    e.undo();
    assert_eq!(e.doc.shapes()[0].font, 1);
}

#[test]
fn the_pen_tail_erases_whatever_tool_is_chosen() {
    let mut e = editor();
    drag(&mut e, &line(p(40., 100.), p(240., 100.), 20), 0);
    assert_eq!(e.doc.shapes().len(), 1);
    let tail = Contact { size: 8.0, stylus: true, eraser: true, norm: 0.0 };
    e.pointer_down_with(3, p(140., 60.), 3000, tail);
    for i in 1..=10 {
        e.pointer_move_with(3, p(140., 60.0 + i as f64 * 8.0), 3000 + i * 8, tail);
    }
    e.pointer_up(3, p(140., 140.), 3100, false);
    assert_eq!(e.doc.shapes().len(), 0);
    assert_eq!(e.tool(), Tool::Pen, "the chosen tool stays");
}

#[test]
fn with_pen_priority_fingers_pan_once_a_stylus_is_used() {
    let pen = Contact { size: 4.0, stylus: true, eraser: false, norm: 0.0 };
    let finger = Contact { size: 14.0, stylus: false, eraser: false, norm: 0.0 };
    let mut e = editor();
    e.set_pen_priority(true);
    // No stylus seen yet: a finger still writes (IR panels report fingers).
    e.pointer_down_with(1, p(50., 50.), 0, finger);
    e.pointer_move_with(1, p(150., 50.), 40, finger);
    e.pointer_up(1, p(150., 50.), 80, false);
    assert_eq!(e.doc.shapes().len(), 1);
    e.pointer_down_with(2, p(50., 120.), 1000, pen);
    e.pointer_move_with(2, p(150., 120.), 1040, pen);
    e.pointer_up(2, p(150., 120.), 1080, false);
    assert_eq!(e.doc.shapes().len(), 2);
    // Now a finger pans the board instead of drawing.
    let before = e.camera();
    e.pointer_down_with(3, p(300., 300.), 3000, finger);
    e.pointer_move_with(3, p(340., 320.), 3040, finger);
    e.pointer_up(3, p(340., 320.), 3080, false);
    assert_eq!(e.doc.shapes().len(), 2);
    assert!((e.camera() - before).len() > 30.0);
}

#[test]
fn several_fingers_write_at_once_in_multi_finger_mode() {
    let mut e = editor();
    e.set_multi_ink(true);
    e.pointer_down(0, p(40., 40.), 0);
    e.pointer_move(0, p(60., 40.), 8);
    e.pointer_down(1, p(40., 200.), 400);
    e.pointer_down(2, p(300., 200.), 410);
    for i in 1..=10 {
        let t = 410 + i * 8;
        let d = i as f64 * 10.0;
        e.pointer_move(0, p(60. + d, 40.), t);
        e.pointer_move(1, p(40. + d, 200.), t);
        e.pointer_move(2, p(300., 200. + d), t);
        e.render();
    }
    e.pointer_up(1, p(140., 200.), 600, false);
    e.pointer_up(0, p(160., 40.), 610, false);
    e.pointer_up(2, p(300., 300.), 620, false);
    e.render();
    assert_eq!(e.doc.shapes().len(), 3);
    assert_eq!(e.zoom(), 1.0);
    assert!(!e.pointer_down_now());
    assert_matches_full_repaint(&mut e);
}

#[test]
fn several_fingers_write_on_an_ir_frame_even_with_wide_erase_on() {
    // The IR frame reports a finger 35 wide, the same as the pen's back nib.
    let finger = Contact { norm: 35.0 / 32767.0, ..Contact::default() };
    let mut e = editor();
    e.set_wide_erases(true);
    e.set_multi_ink(true);
    e.pointer_down_with(0, p(40., 40.), 0, finger);
    e.pointer_down_with(1, p(40., 300.), 30, finger);
    e.pointer_down_with(2, p(500., 300.), 40, finger);
    for i in 1..=10 {
        let t = 40 + i * 8;
        let d = i as f64 * 10.0;
        e.pointer_move_with(0, p(40. + d, 40.), t, finger);
        e.pointer_move_with(1, p(40. + d, 300.), t, finger);
        e.pointer_move_with(2, p(500., 300. + d), t, finger);
    }
    e.pointer_up(0, p(140., 40.), 200, false);
    e.pointer_up(1, p(140., 300.), 210, false);
    e.pointer_up(2, p(500., 400.), 220, false);
    assert_eq!(e.doc.shapes().len(), 3);

    // With several-finger writing off, a wide touch still erases.
    e.set_multi_ink(false);
    e.pointer_down_with(0, p(90., 30.), 1000, finger);
    for i in 1..=10 {
        e.pointer_move_with(0, p(90., 30. + i as f64 * 3.0), 1000 + i * 8, finger);
    }
    e.pointer_up(0, p(90., 60.), 1100, false);
    assert_eq!(e.doc.shapes().len(), 2);
}

#[test]
fn prediction_runs_ahead_while_writing_and_leaves_no_trace() {
    let mut e = editor();
    e.set_prediction(24.0);
    e.pointer_down(0, p(40., 100.), 0);
    for i in 1..=30 {
        e.pointer_move(0, p(40. + i as f64 * 6.0, 100.), i * 8);
        e.render();
    }
    // Ink is visible beyond the last real sample (x = 220).
    let f = e.frame();
    let ahead = (226..250).any(|x| {
        let o = (100 * f.w as usize + x) * 4;
        f.data[o] < 128
    });
    assert!(ahead, "the stroke should be drawn ahead of the pen");
    e.pointer_up(0, p(220., 100.), 31 * 8, false);
    e.render();
    assert!(e.doc.shapes()[0].bounds().r <= 221.0);
    assert_matches_full_repaint(&mut e);
}

#[test]
fn a_hand_resting_before_writing_is_ignored() {
    // The palm reports a finger-sized contact, lands first and stays put.
    let palm = Contact { size: 30.0, stylus: false, eraser: false, norm: 0.0 };
    let finger = Contact { size: 14.0, stylus: false, eraser: false, norm: 0.0 };
    let mut e = editor();
    e.pointer_down_with(5, p(300., 250.), 0, palm);
    e.pointer_move_with(5, p(303., 252.), 150, palm);
    e.pointer_down_with(0, p(50., 50.), 600, finger); // then the writer
    for i in 1..=20 {
        e.pointer_move_with(0, p(50.0 + i as f64 * 5.0, 50.), 600 + i * 8, finger);
        e.pointer_move_with(5, p(303., 252. + (i % 3) as f64), 600 + i * 8, palm);
    }
    e.pointer_up(0, p(150., 50.), 800, false);
    // A second word while the hand still rests.
    e.pointer_down_with(1, p(50., 80.), 900, finger);
    e.pointer_move_with(1, p(120., 80.), 950, finger);
    e.pointer_up(1, p(120., 80.), 1000, false);
    e.pointer_up(5, p(303., 254.), 1100, false);
    e.render();
    assert_eq!(e.doc.shapes().len(), 2, "both words, and no dot from the palm");
    assert!(e.doc.shapes().iter().all(|s| s.bounds().b < 90.0));
    assert!(!e.pointer_down_now());
    assert_matches_full_repaint(&mut e);

    // The same with several writers allowed: the palm is not a writer.
    let mut e = editor();
    e.set_multi_ink(true);
    e.pointer_down_with(5, p(300., 250.), 0, palm);
    e.pointer_down_with(0, p(50., 50.), 500, finger);
    e.pointer_move_with(0, p(150., 50.), 550, finger);
    e.pointer_up(0, p(150., 50.), 600, false);
    e.pointer_up(5, p(300., 250.), 700, false);
    assert_eq!(e.doc.shapes().len(), 1);
    assert!(e.doc.shapes()[0].bounds().b < 60.0);

    // A palm that lands as two touches looks like a pinch: writing after it
    // undoes the zoom and draws.
    let mut e = editor();
    e.pointer_down_with(5, p(300., 250.), 0, palm);
    e.pointer_down_with(6, p(330., 260.), 40, palm);
    e.pointer_move_with(6, p(334., 262.), 80, palm);
    e.pointer_down_with(0, p(50., 50.), 500, finger);
    e.pointer_move_with(0, p(150., 50.), 550, finger);
    e.pointer_up(0, p(150., 50.), 600, false);
    e.pointer_up(5, p(300., 250.), 700, false);
    e.pointer_up(6, p(334., 262.), 700, false);
    assert_eq!(e.zoom(), 1.0);
    assert_eq!(e.doc.shapes().len(), 1);

    // A hand that rests and lifts without writing leaves nothing.
    let mut e = editor();
    e.pointer_down_with(5, p(300., 250.), 0, palm);
    e.pointer_move_with(5, p(301., 251.), 500, palm);
    e.pointer_up(5, p(301., 251.), 1200, false);
    assert_eq!(e.doc.shapes().len(), 0);

    // A quick tap is still a dot, and a finger that pauses mid-stroke keeps it.
    let mut e = editor();
    e.pointer_down_with(0, p(50., 50.), 0, finger);
    e.pointer_up(0, p(50., 50.), 80, false);
    e.pointer_down_with(1, p(60., 100.), 200, finger);
    e.pointer_move_with(1, p(160., 100.), 260, finger);
    e.pointer_down_with(2, p(300., 250.), 900, palm); // lands after a pause
    e.pointer_move_with(1, p(200., 100.), 950, finger);
    e.pointer_up(2, p(300., 250.), 980, false);
    e.pointer_up(1, p(200., 100.), 1000, false);
    assert_eq!(e.doc.shapes().len(), 2);
}

#[test]
fn slow_jittery_ink_is_smoothed_and_ends_where_the_finger_lifts() {
    let finger = Contact { size: 14.0, stylus: false, eraser: false, norm: 0.0 };
    let draw = |e: &mut Editor| {
        e.pointer_down_with(0, p(50., 100.), 0, finger);
        // Slow writing on a panel that wobbles by ±2 px.
        for i in 1..=60u64 {
            let y = if i % 2 == 0 { 102.0 } else { 98.0 };
            e.pointer_move_with(0, p(50.0 + i as f64 * 2.0, y), i * 8, finger);
        }
        e.pointer_up(0, p(170., 100.), 490, false);
        e.doc.shapes()[0].points.clone()
    };
    let wobble = |pts: &[Pt]| pts[5..pts.len() - 1].iter().map(|q| (q.y - 100.0).abs()).fold(0.0, f64::max);
    let mut raw = editor();
    raw.set_smoothing(false);
    let rough = draw(&mut raw);
    let mut e = editor();
    assert!(e.smoothing());
    let smooth = draw(&mut e);
    assert!(wobble(&rough) >= 2.0);
    assert!(wobble(&smooth) < 1.0, "wobble {}", wobble(&smooth));
    assert_eq!(*smooth.last().unwrap(), p(170., 100.));
}

/// The IR panel reports every contact at one size, so the hand is told apart
/// by where it lands and how it moves.
#[test]
fn a_palm_landing_with_the_pen_is_ignored() {
    let mut e = editor();
    e.pointer_down(0, p(300., 200.), 0); // pen
    e.pointer_down(1, p(350., 260.), 20); // palm, below right, with it
    for i in 1..=30u64 {
        e.pointer_move(0, p(300.0 + i as f64 * 6.0, 200.0 + (i % 5) as f64 * 4.0), 20 + i * 10);
        e.pointer_move(1, p(350.0 + i as f64 * 1.0, 260.), 20 + i * 10);
    }
    e.pointer_up(1, p(380., 260.), 330, false);
    e.pointer_up(0, p(480., 200.), 340, false);
    assert_eq!(e.zoom(), 1.0, "no accidental zoom");
    assert_eq!(e.doc.shapes().len(), 1);
    assert_eq!(e.doc.shapes()[0].points[0], p(300., 200.), "ink starts where the pen landed");
}

#[test]
fn a_palm_that_lands_first_and_drags_gives_way_to_the_pen() {
    let mut e = editor();
    e.pointer_down(0, p(400., 300.), 0); // palm, moving along
    for i in 1..=20u64 {
        e.pointer_move(0, p(400.0 + i as f64 * 3.0, 300.), i * 15);
    }
    e.pointer_down(1, p(380., 230.), 320); // pen, above it
    for i in 1..=20u64 {
        e.pointer_move(1, p(380.0 + i as f64 * 8.0, 230.), 320 + i * 10);
        e.pointer_move(0, p(460.0 + i as f64 * 2.0, 300.), 320 + i * 10);
    }
    e.pointer_up(1, p(540., 230.), 530, false);
    e.pointer_up(0, p(500., 300.), 540, false);
    assert_eq!(e.doc.shapes().len(), 1, "only the pen writes");
    assert_eq!(e.doc.shapes()[0].points[0], p(380., 230.));
}

#[test]
fn two_writers_still_work_but_a_palm_beside_one_does_not() {
    let mut e = editor();
    e.set_multi_ink(true);
    e.pointer_down(0, p(100., 100.), 0);
    e.pointer_move(0, p(140., 100.), 300);
    e.pointer_down(1, p(700., 100.), 310); // a second writer, far away
    e.pointer_move(1, p(760., 100.), 400);
    e.pointer_down(2, p(150., 160.), 410); // the first writer's palm
    e.pointer_move(2, p(160., 160.), 420);
    e.pointer_up(2, p(160., 160.), 430, false);
    e.pointer_up(1, p(760., 100.), 440, false);
    e.pointer_up(0, p(180., 100.), 450, false);
    assert_eq!(e.doc.shapes().len(), 2);
}

fn ir(raw: f64) -> Contact {
    Contact { size: 2.0, stylus: false, eraser: false, norm: raw / 32767.0 }
}

#[test]
fn the_pens_back_nib_erases_on_an_ir_panel() {
    let mut e = editor();
    e.pointer_down_with(0, p(100., 100.), 0, ir(15.));
    e.pointer_move_with(0, p(300., 100.), 50, ir(15.));
    e.pointer_up(0, p(300., 100.), 60, false);
    assert_eq!(e.doc.shapes().len(), 1, "the tip writes");
    // The nib can land edge first (tip sized) and widen a moment later.
    e.pointer_down_with(1, p(200., 60.), 1000, ir(15.));
    e.pointer_move_with(1, p(200., 80.), 1020, ir(35.));
    e.pointer_move_with(1, p(200., 140.), 1040, ir(35.));
    e.pointer_up(1, p(200., 140.), 1060, false);
    assert_eq!(e.doc.shapes().len(), 0, "the nib erased the stroke without drawing");
}

#[test]
fn a_wide_ir_contact_is_a_palm_even_after_it_moved() {
    let mut e = editor();
    e.pointer_down_with(0, p(100., 300.), 0, ir(70.));
    for i in 1..=40u64 {
        e.pointer_move_with(0, p(100.0 + i as f64 * 5.0, 300.), i * 20, ir(if i < 30 { 70. } else { 520. }));
    }
    e.pointer_up(0, p(300., 300.), 820, false);
    assert_eq!(e.doc.shapes().len(), 0);
    e.pointer_down_with(1, p(100., 100.), 1000, ir(15.)); // a finger still writes
    e.pointer_move_with(1, p(200., 100.), 1050, ir(15.));
    e.pointer_up(1, p(200., 100.), 1060, false);
    assert_eq!(e.doc.shapes().len(), 1);
}

/// Replays a capture from the Aaztec IR frame (`down|move|up id x y ms raw`,
/// logical pixels) and returns how many shapes it left.
fn replay(name: &str, seed: Option<&str>) -> (usize, usize) {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let mut e = Editor::new();
    e.resize(1280, 720, 1.0);
    e.set_grid(false);
    e.set_auto_convert(false);
    e.set_auto_text(false);
    e.render();
    if let Some(seed) = seed {
        let rows: Vec<Vec<f64>> = std::fs::read_to_string(format!("{}/tests/fixtures/{seed}", env!("CARGO_MANIFEST_DIR")))
            .unwrap()
            .lines()
            .map(|l| l.split_whitespace().skip(2).take(2).map(|v| v.parse().unwrap()).collect())
            .collect();
        // Ink under the whole path so the nib has something to erase.
        for (i, r) in rows.iter().step_by(40).enumerate() {
            let at = p(r[0], r[1]);
            e.pointer_down_with(9000 + i as i32, at - p(20., 0.), 0, ir(15.));
            e.pointer_move_with(9000 + i as i32, at + p(20., 0.), 30, ir(15.));
            e.pointer_up(9000 + i as i32, at + p(20., 0.), 40, false);
        }
    }
    let seeded = e.doc.shapes().len();
    let base = 100_000;
    for line in std::fs::read_to_string(path).unwrap().lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let id: i32 = f[1].parse().unwrap();
        let at = p(f[2].parse().unwrap(), f[3].parse().unwrap());
        let t = base + f[4].parse::<u64>().unwrap();
        let c = ir(f[5].parse().unwrap());
        match f[0] {
            "down" => e.pointer_down_with(id, at, t, c),
            "move" => e.pointer_move_with(id, at, t, c),
            _ => {
                e.pointer_up(id, at, t, false);
            }
        }
    }
    (seeded, e.doc.shapes().len())
}

#[test]
fn a_recorded_palm_and_pen_leave_only_the_pen_strokes() {
    // 12 pen-tip strokes (raw 15) written with the palm resting (raw 150-650).
    assert_eq!(replay("ir_palm_and_pen.txt", None).1, 12);
}

#[test]
fn a_recorded_back_nib_erases_and_never_draws() {
    assert_eq!(replay("ir_back_nib.txt", None).1, 0);
    let (seeded, left) = replay("ir_back_nib.txt", Some("ir_back_nib.txt"));
    assert!(seeded > 20 && left * 2 < seeded, "the nib erased the ink under it: {left} of {seeded} left");
}


#[test]
fn two_ir_pens_write_together_but_a_ghost_beside_one_does_not() {
    let mut e = editor();
    e.set_multi_ink(true);
    e.pointer_down_with(0, p(100., 100.), 0, ir(15.));
    e.pointer_move_with(0, p(140., 100.), 300, ir(15.));
    e.pointer_down_with(1, p(700., 100.), 310, ir(15.)); // a second pen, far away
    e.pointer_move_with(1, p(760., 100.), 400, ir(15.));
    e.pointer_down_with(2, p(150., 130.), 410, ir(15.)); // beside the first pen
    e.pointer_move_with(2, p(190., 130.), 420, ir(15.));
    e.pointer_up(2, p(190., 130.), 430, false);
    e.pointer_up(1, p(760., 100.), 440, false);
    e.pointer_up(0, p(180., 100.), 450, false);
    assert_eq!(e.doc.shapes().len(), 2);
}

#[test]
fn with_wide_erase_off_a_finger_draws_and_two_fingers_pinch() {
    let mut e = editor();
    e.set_wide_erases(false);
    e.pointer_down_with(0, p(100., 100.), 0, ir(35.));
    e.pointer_move_with(0, p(300., 100.), 50, ir(35.));
    e.pointer_up(0, p(300., 100.), 60, false);
    assert_eq!(e.doc.shapes().len(), 1, "the finger writes");
    let zoom = e.zoom();
    e.pointer_down_with(1, p(400., 300.), 1000, ir(35.));
    e.pointer_down_with(2, p(500., 300.), 1010, ir(35.));
    for i in 1..=10u64 {
        e.pointer_move_with(1, p(400. - i as f64 * 10., 300.), 1010 + i * 16, ir(35.));
        e.pointer_move_with(2, p(500. + i as f64 * 10., 300.), 1010 + i * 16, ir(35.));
    }
    e.pointer_up(1, p(300., 300.), 1200, false);
    e.pointer_up(2, p(600., 300.), 1200, false);
    assert!(e.zoom() > zoom * 1.5, "two fingers zoom");
    assert_eq!(e.doc.shapes().len(), 1);
}

/// A hand-drawn closed outline: `corner` rounds the corners of a w x h box
/// (0 = sharp, 1 = an ellipse), with a steady wobble and a small gap.
fn sketched(w: f64, h: f64, corner: f64, wobble: f64) -> Shape {
    let (rx, ry) = (w / 2.0, h / 2.0);
    let n = 160;
    ink((0..n - 3)
        .map(|i| {
            let a = i as f64 * PI * 2.0 / n as f64;
            let (c, s) = (a.cos(), a.sin());
            // Superellipse: exponent 2 is an ellipse, large is a box.
            let e = 2.0 + (1.0 - corner) * 10.0;
            let k = (c.abs().powf(e) + s.abs().powf(e)).powf(-1.0 / e);
            let jitter = 1.0 + wobble * (a * 7.0).sin() + wobble * 0.5 * (a * 13.0).cos();
            p(200.0 + c * k * rx * jitter, 200.0 + s * k * ry * jitter)
        })
        .collect())
}

#[test]
fn a_rough_box_with_rounded_corners_stays_a_box_not_a_circle() {
    for (w, h) in [(200., 200.), (300., 140.)] {
        for corner in [0.0, 0.3, 0.5] {
            let tool = recognize_shape(&sketched(w, h, corner, 0.03)).map(|s| s.tool);
            assert_ne!(tool, Some(Tool::Ellipse), "{w}x{h} box, corner {corner}");
        }
        let tool = recognize_shape(&sketched(w, h, 0.0, 0.03)).map(|s| s.tool);
        assert_eq!(tool, Some(Tool::Rectangle), "{w}x{h} rough box");
    }
}

#[test]
fn a_wobbly_circle_or_ellipse_is_still_recognized() {
    for (w, h) in [(200., 200.), (300., 160.)] {
        let tool = recognize_shape(&sketched(w, h, 1.0, 0.04)).map(|s| s.tool);
        assert_eq!(tool, Some(Tool::Ellipse), "{w}x{h}");
    }
}
