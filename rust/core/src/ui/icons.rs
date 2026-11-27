//! The Inkboard icon set, drawn on a 24 by 24 grid.
//!
//! All stroked geometry keeps at least a 2.5-unit inset from the 24x24 edges
//! so 2-unit strokes stay crisp and never clip at small sizes.

use super::paint::{push_polyline, rrect_path, Rf};
use tiny_skia::{Path, PathBuilder};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ico {
    Back,
    More,
    Kebab,
    Share,
    Undo,
    Redo,
    Select,
    Hand,
    Pen,
    Highlighter,
    Eraser,
    Shapes,
    Text,
    Convert,
    Refine,
    Plus,
    Minus,
    Fit,
    Close,
    Check,
    Chevron,
    ChevronLeft,
    Gear,
    Join,
    Claude,
    Export,
    Import,
    Update,
    Font,
    Rect,
    Ellipse,
    Arrow,
    Line,
    Triangle,
    Diamond,
    Star,
    Hexagon,
    Smart,
    Trash,
    Rename,
    Copy,
    Grid,
    Dot,
    Bigger,
    Smaller,
    Page,
    PagePlus,
    Table,
    MindMap,
    Formula,
    Image,
    Pdf,
    Palette,
    Dock,
    Graph,
    Lock,
    People,
    Globe,
    Link,
    Exit,
}

pub const ALL: [Ico; 60] = [
    Ico::Back,
    Ico::More,
    Ico::Kebab,
    Ico::Share,
    Ico::Undo,
    Ico::Redo,
    Ico::Select,
    Ico::Hand,
    Ico::Pen,
    Ico::Highlighter,
    Ico::Eraser,
    Ico::Shapes,
    Ico::Text,
    Ico::Convert,
    Ico::Refine,
    Ico::Plus,
    Ico::Minus,
    Ico::Fit,
    Ico::Close,
    Ico::Check,
    Ico::Chevron,
    Ico::ChevronLeft,
    Ico::Gear,
    Ico::Join,
    Ico::Claude,
    Ico::Export,
    Ico::Import,
    Ico::Update,
    Ico::Font,
    Ico::Rect,
    Ico::Ellipse,
    Ico::Arrow,
    Ico::Line,
    Ico::Triangle,
    Ico::Diamond,
    Ico::Star,
    Ico::Hexagon,
    Ico::Smart,
    Ico::Trash,
    Ico::Rename,
    Ico::Copy,
    Ico::Grid,
    Ico::Dot,
    Ico::Bigger,
    Ico::Smaller,
    Ico::Page,
    Ico::PagePlus,
    Ico::Table,
    Ico::MindMap,
    Ico::Formula,
    Ico::Image,
    Ico::Pdf,
    Ico::Palette,
    Ico::Dock,
    Ico::Graph,
    Ico::Lock,
    Ico::People,
    Ico::Globe,
    Ico::Link,
    Ico::Exit,
];

impl Ico {
    pub const ALL: [Ico; 60] = ALL;
}

fn outline(path: &mut PathBuilder, pts: &[(f32, f32)]) {
    push_polyline(path, pts);
    path.close();
}

fn rounded_rect(path: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, r: f32) {
    if let Some(rect) = rrect_path(Rf::new(x, y, w, h), r) {
        path.push_path(&rect);
    }
}

/// Smooth curved 4-point sparkle centred at `(cx, cy)`.
fn sparkle(path: &mut PathBuilder, cx: f32, cy: f32, r: f32) {
    let c = r * 0.22;
    path.move_to(cx, cy - r);
    path.cubic_to(cx + c, cy - c, cx + c, cy - c, cx + r, cy);
    path.cubic_to(cx + c, cy + c, cx + c, cy + c, cx, cy + r);
    path.cubic_to(cx - c, cy + c, cx - c, cy + c, cx - r, cy);
    path.cubic_to(cx - c, cy - c, cx - c, cy - c, cx, cy - r);
    path.close();
}

fn pen_outline(path: &mut PathBuilder) {
    path.move_to(4.0, 20.0);
    path.line_to(5.4, 15.6);
    path.line_to(15.6, 5.4);
    path.cubic_to(16.6, 4.4, 18.2, 4.4, 19.2, 5.4);
    path.cubic_to(20.2, 6.4, 20.2, 8.0, 19.2, 9.0);
    path.line_to(9.0, 19.2);
    path.close();
}

fn hand_outline(path: &mut PathBuilder) {
    // Clean open-palm grab hand with balanced finger spacing and thumb.
    path.move_to(8.5, 11.5);
    path.line_to(8.5, 6.2);
    path.cubic_to(8.5, 4.6, 11.0, 4.6, 11.0, 6.2);
    path.line_to(11.0, 11.0);
    path.line_to(11.0, 4.6);
    path.cubic_to(11.0, 3.0, 13.5, 3.0, 13.5, 4.6);
    path.line_to(13.5, 11.0);
    path.line_to(13.5, 5.6);
    path.cubic_to(13.5, 4.0, 16.0, 4.0, 16.0, 5.6);
    path.line_to(16.0, 12.0);
    path.line_to(16.0, 8.6);
    path.cubic_to(16.0, 7.0, 18.5, 7.0, 18.5, 8.6);
    path.line_to(18.5, 14.5);
    path.cubic_to(18.5, 18.2, 15.8, 20.5, 12.5, 20.5);
    path.line_to(11.5, 20.5);
    path.cubic_to(9.5, 20.5, 8.0, 19.6, 6.9, 18.0);
    path.line_to(4.6, 14.2);
    path.cubic_to(3.7, 12.8, 5.4, 11.2, 6.7, 12.2);
    path.line_to(8.5, 13.6);
    path.close();
}

fn history_arrow(path: &mut PathBuilder, mirrored: bool) {
    let x = |x: f32| if mirrored { 24.0 - x } else { x };
    // Sweeping circular return arrow.
    push_polyline(path, &[(x(8.8), 6.0), (x(4.5), 10.0), (x(8.8), 14.0)]);
    path.move_to(x(5.0), 10.0);
    path.line_to(x(14.0), 10.0);
    path.cubic_to(x(17.2), 10.0, x(19.5), 12.2, x(19.5), 15.0);
    path.cubic_to(x(19.5), 17.8, x(17.2), 20.0, x(14.0), 20.0);
    path.line_to(x(10.5), 20.0);
}

fn gear_outline(path: &mut PathBuilder) {
    // 8-tooth sculpted gear wheel: continuous ring with rounded teeth.
    let n = 8;
    let r_in = 6.2_f32;
    let r_out = 8.9_f32;
    let step = std::f32::consts::TAU / n as f32;
    let pt = |angle: f32, r: f32| {
        let (s, c) = angle.sin_cos();
        (12.0 + r * c, 12.0 + r * s)
    };
    let mut started = false;
    for i in 0..n {
        let base = i as f32 * step;
        let a0 = base - step * 0.24;
        let a1 = base - step * 0.14;
        let a2 = base + step * 0.14;
        let a3 = base + step * 0.24;
        let (x0, y0) = pt(a0, r_in);
        let (x1, y1) = pt(a1, r_out);
        let (x2, y2) = pt(a2, r_out);
        let (x3, y3) = pt(a3, r_in);
        if !started {
            path.move_to(x0, y0);
            started = true;
        } else {
            path.line_to(x0, y0);
        }
        path.line_to(x1, y1);
        path.line_to(x2, y2);
        path.line_to(x3, y3);
    }
    path.close();
}

pub(super) fn paths(ico: Ico) -> (Option<Path>, Option<Path>) {
    use Ico::*;

    let mut stroke = PathBuilder::new();
    let mut fill = PathBuilder::new();
    match ico {
        Back => {
            push_polyline(&mut stroke, &[(19.0, 12.0), (5.5, 12.0)]);
            push_polyline(&mut stroke, &[(11.0, 6.5), (5.5, 12.0), (11.0, 17.5)]);
        }
        More | Kebab => {
            for p in [5.5, 12.0, 18.5] {
                let (x, y) = if ico == More { (p, 12.0) } else { (12.0, p) };
                fill.push_circle(x, y, 1.7);
            }
        }
        Share => {
            for (x, y) in [(6.0, 12.0), (17.5, 6.0), (17.5, 18.0)] {
                stroke.push_circle(x, y, 2.5);
            }
            push_polyline(&mut stroke, &[(8.2, 10.8), (15.3, 7.2)]);
            push_polyline(&mut stroke, &[(8.2, 13.2), (15.3, 16.8)]);
        }
        Undo | Redo => history_arrow(&mut stroke, ico == Redo),
        Select => {
            // Crisp Figma-style cursor arrow.
            outline(
                &mut stroke,
                &[(5.5, 3.8), (18.8, 10.6), (13.0, 12.6), (10.6, 18.8)],
            );
        }
        Hand => hand_outline(&mut stroke),
        Pen | Rename => {
            pen_outline(&mut stroke);
            if ico == Pen {
                push_polyline(&mut stroke, &[(13.6, 7.4), (17.2, 11.0)]);
            } else {
                push_polyline(&mut stroke, &[(13.0, 20.0), (20.5, 20.0)]);
            }
        }
        Highlighter => {
            // Angled chisel-tip highlighter marker with highlight bar.
            outline(
                &mut stroke,
                &[(8.6, 13.4), (15.8, 6.2), (19.3, 9.7), (12.1, 16.9)],
            );
            // Chisel nib tapering to bottom-left.
            push_polyline(&mut stroke, &[(8.6, 13.4), (6.4, 17.6), (10.2, 17.6), (12.1, 16.9)]);
            // Collar line.
            push_polyline(&mut stroke, &[(13.6, 8.4), (17.1, 11.9)]);
            // Highlighted stroke underneath.
            push_polyline(&mut stroke, &[(4.5, 20.5), (15.5, 20.5)]);
        }
        Eraser => {
            // Sleek tilted block eraser with rounded corners and sleeve band.
            stroke.move_to(7.8, 19.2);
            stroke.line_to(4.8, 16.2);
            stroke.quad_to(3.8, 15.2, 4.8, 14.2);
            stroke.line_to(12.6, 6.4);
            stroke.quad_to(13.6, 5.4, 14.6, 6.4);
            stroke.line_to(19.2, 11.0);
            stroke.quad_to(20.2, 12.0, 19.2, 13.0);
            stroke.line_to(13.0, 19.2);
            stroke.close();
            // Sleeve divider.
            push_polyline(&mut stroke, &[(8.8, 10.2), (14.4, 15.8)]);
            // Subtle baseline.
            push_polyline(&mut stroke, &[(7.8, 19.2), (19.5, 19.2)]);
        }
        Shapes => {
            // Balanced geometric triad: circle, triangle, rounded square.
            stroke.push_circle(7.8, 8.2, 3.8);
            outline(&mut stroke, &[(16.8, 4.4), (20.6, 11.4), (13.0, 11.4)]);
            rounded_rect(&mut stroke, 8.2, 13.4, 7.6, 7.0, 1.8);
        }
        Text => {
            push_polyline(&mut stroke, &[(5.5, 7.0), (5.5, 4.8), (18.5, 4.8), (18.5, 7.0)]);
            push_polyline(&mut stroke, &[(12.0, 4.8), (12.0, 19.5)]);
            push_polyline(&mut stroke, &[(9.0, 19.5), (15.0, 19.5)]);
        }
        Convert => {
            // Crisp serif "T" paired with a diagonal stylus nib (handwriting to text).
            push_polyline(&mut stroke, &[(4.5, 6.8), (4.5, 5.0), (16.5, 5.0), (16.5, 6.8)]);
            push_polyline(&mut stroke, &[(10.5, 5.0), (10.5, 18.8)]);
            push_polyline(&mut stroke, &[(8.0, 18.8), (13.0, 18.8)]);
            outline(
                &mut stroke,
                &[(14.2, 19.2), (15.0, 16.4), (18.6, 12.8), (20.2, 14.4), (16.6, 18.0)],
            );
        }
        Refine => {
            // Flowing cursive wave settling onto a crisp baseline + sparkle.
            stroke.move_to(3.2, 12.6);
            stroke.cubic_to(5.0, 7.8, 7.2, 15.8, 9.6, 10.6);
            stroke.cubic_to(11.4, 7.4, 13.2, 14.4, 15.4, 11.4);
            push_polyline(&mut stroke, &[(3.2, 17.5), (17.5, 17.5)]);
            sparkle(&mut fill, 18.5, 6.2, 3.2);
        }
        Plus | Minus => {
            push_polyline(&mut stroke, &[(5.5, 12.0), (18.5, 12.0)]);
            if ico == Plus {
                push_polyline(&mut stroke, &[(12.0, 5.5), (12.0, 18.5)]);
            }
        }
        Fit => {
            // Rounded corner brackets.
            stroke.move_to(4.2, 9.0);
            stroke.line_to(4.2, 6.2);
            stroke.quad_to(4.2, 4.2, 6.2, 4.2);
            stroke.line_to(9.0, 4.2);

            stroke.move_to(15.0, 4.2);
            stroke.line_to(17.8, 4.2);
            stroke.quad_to(19.8, 4.2, 19.8, 6.2);
            stroke.line_to(19.8, 9.0);

            stroke.move_to(4.2, 15.0);
            stroke.line_to(4.2, 17.8);
            stroke.quad_to(4.2, 19.8, 6.2, 19.8);
            stroke.line_to(9.0, 19.8);

            stroke.move_to(15.0, 19.8);
            stroke.line_to(17.8, 19.8);
            stroke.quad_to(19.8, 19.8, 19.8, 17.8);
            stroke.line_to(19.8, 15.0);
        }
        Close => {
            push_polyline(&mut stroke, &[(6.2, 6.2), (17.8, 17.8)]);
            push_polyline(&mut stroke, &[(17.8, 6.2), (6.2, 17.8)]);
        }
        Check => push_polyline(&mut stroke, &[(5.0, 12.4), (9.6, 17.0), (19.0, 7.4)]),
        Chevron => push_polyline(&mut stroke, &[(9.2, 5.8), (15.4, 12.0), (9.2, 18.2)]),
        ChevronLeft => push_polyline(&mut stroke, &[(14.8, 5.8), (8.6, 12.0), (14.8, 18.2)]),
        Gear => {
            gear_outline(&mut stroke);
            stroke.push_circle(12.0, 12.0, 2.8);
        }
        Join => {
            stroke.push_circle(9.2, 7.8, 3.2);
            stroke.move_to(3.5, 19.5);
            stroke.cubic_to(3.5, 16.0, 6.0, 13.8, 9.2, 13.8);
            stroke.cubic_to(12.4, 13.8, 14.9, 16.0, 14.9, 19.5);
            push_polyline(&mut stroke, &[(18.5, 8.2), (18.5, 13.8)]);
            push_polyline(&mut stroke, &[(15.7, 11.0), (21.3, 11.0)]);
        }
        Claude => {
            // Graceful curved 4-point sparkle with a tiny companion accent.
            sparkle(&mut stroke, 11.2, 12.4, 7.6);
            sparkle(&mut fill, 18.8, 5.6, 2.5);
        }
        Export | Import | Update => {
            if ico == Update {
                push_polyline(&mut stroke, &[(5.5, 19.5), (18.5, 19.5)]);
            } else {
                stroke.move_to(4.5, 14.8);
                stroke.line_to(4.5, 17.8);
                stroke.quad_to(4.5, 19.5, 6.2, 19.5);
                stroke.line_to(17.8, 19.5);
                stroke.quad_to(19.5, 19.5, 19.5, 17.8);
                stroke.line_to(19.5, 14.8);
            }
            push_polyline(&mut stroke, &[(12.0, 4.2), (12.0, 15.2)]);
            if ico == Export {
                push_polyline(&mut stroke, &[(7.5, 8.7), (12.0, 4.2), (16.5, 8.7)]);
            } else {
                push_polyline(&mut stroke, &[(7.5, 10.7), (12.0, 15.2), (16.5, 10.7)]);
            }
        }
        Font => {
            push_polyline(&mut stroke, &[(3.5, 18.8), (8.2, 6.0), (12.9, 18.8)]);
            push_polyline(&mut stroke, &[(5.2, 14.5), (11.2, 14.5)]);
            // Secondary smaller 'A' for crisp typography pairing.
            push_polyline(&mut stroke, &[(14.6, 18.8), (17.8, 10.6), (21.0, 18.8)]);
            push_polyline(&mut stroke, &[(15.7, 16.2), (19.9, 16.2)]);
        }
        Rect => rounded_rect(&mut stroke, 4.0, 6.0, 16.0, 12.0, 2.2),
        Ellipse => {
            if let Some(oval) = tiny_skia::Rect::from_xywh(3.5, 5.8, 17.0, 12.4) {
                stroke.push_oval(oval);
            }
        }
        Arrow | Line => {
            push_polyline(&mut stroke, &[(5.2, 18.8), (18.8, 5.2)]);
            if ico == Arrow {
                push_polyline(&mut stroke, &[(10.5, 5.2), (18.8, 5.2), (18.8, 13.5)]);
            }
        }
        Triangle => outline(&mut stroke, &[(12.0, 4.4), (20.2, 19.2), (3.8, 19.2)]),
        Diamond => outline(
            &mut stroke,
            &[(12.0, 3.8), (20.2, 12.0), (12.0, 20.2), (3.8, 12.0)],
        ),
        Star => outline(
            &mut stroke,
            &[
                (12.0, 3.8),
                (14.5, 8.8),
                (20.1, 9.6),
                (16.0, 13.6),
                (17.0, 19.2),
                (12.0, 16.5),
                (7.0, 19.2),
                (8.0, 13.6),
                (3.9, 9.6),
                (9.5, 8.8),
            ],
        ),
        Hexagon => outline(
            &mut stroke,
            &[
                (7.8, 4.6),
                (16.2, 4.6),
                (20.4, 12.0),
                (16.2, 19.4),
                (7.8, 19.4),
                (3.6, 12.0),
            ],
        ),
        Smart => {
            // Smooth hand-drawn wave turning into a clean rounded shape + sparkle.
            stroke.move_to(3.2, 16.0);
            stroke.cubic_to(4.6, 9.6, 7.0, 15.8, 9.2, 11.4);
            rounded_rect(&mut stroke, 11.8, 8.2, 8.8, 8.8, 2.0);
            sparkle(&mut fill, 18.6, 5.0, 2.4);
        }
        Trash => {
            push_polyline(&mut stroke, &[(4.5, 7.0), (19.5, 7.0)]);
            stroke.move_to(9.0, 7.0);
            stroke.line_to(9.0, 5.2);
            stroke.quad_to(9.0, 4.2, 10.0, 4.2);
            stroke.line_to(14.0, 4.2);
            stroke.quad_to(15.0, 4.2, 15.0, 5.2);
            stroke.line_to(15.0, 7.0);
            stroke.move_to(6.2, 7.0);
            stroke.line_to(7.0, 18.0);
            stroke.quad_to(7.1, 19.8, 9.0, 19.8);
            stroke.line_to(15.0, 19.8);
            stroke.quad_to(16.9, 19.8, 17.0, 18.0);
            stroke.line_to(17.8, 7.0);
            push_polyline(&mut stroke, &[(10.2, 10.8), (10.2, 16.4)]);
            push_polyline(&mut stroke, &[(13.8, 10.8), (13.8, 16.4)]);
        }
        Copy => {
            rounded_rect(&mut stroke, 8.4, 8.4, 11.4, 11.4, 2.2);
            stroke.move_to(15.8, 8.4);
            stroke.line_to(15.8, 5.8);
            stroke.quad_to(15.8, 4.2, 14.2, 4.2);
            stroke.line_to(5.8, 4.2);
            stroke.quad_to(4.2, 4.2, 4.2, 5.8);
            stroke.line_to(4.2, 14.2);
            stroke.quad_to(4.2, 15.8, 5.8, 15.8);
            stroke.line_to(8.4, 15.8);
        }
        Grid => {
            // 3x3 dot grid matching Inkboard's dot-grid canvas.
            for y in [6.0, 12.0, 18.0] {
                for x in [6.0, 12.0, 18.0] {
                    fill.push_circle(x, y, 1.5);
                }
            }
        }
        Dot => fill.push_circle(12.0, 12.0, 6.8),
        Bigger | Smaller => {
            push_polyline(&mut stroke, &[(3.6, 18.8), (8.4, 5.6), (13.2, 18.8)]);
            push_polyline(&mut stroke, &[(5.3, 14.4), (11.5, 14.4)]);
            push_polyline(&mut stroke, &[(18.2, 7.2), (18.2, 16.8)]);
            let (wings, tip) = if ico == Bigger {
                (10.0, 7.2)
            } else {
                (14.0, 16.8)
            };
            push_polyline(&mut stroke, &[(15.6, wings), (18.2, tip), (20.8, wings)]);
        }
        Page | PagePlus | Pdf => {
            stroke.move_to(7.0, 3.8);
            stroke.line_to(13.8, 3.8);
            stroke.line_to(18.8, 8.8);
            stroke.line_to(18.8, 18.4);
            stroke.quad_to(18.8, 20.2, 17.0, 20.2);
            stroke.line_to(7.0, 20.2);
            stroke.quad_to(5.2, 20.2, 5.2, 18.4);
            stroke.line_to(5.2, 5.6);
            stroke.quad_to(5.2, 3.8, 7.0, 3.8);
            stroke.close();
            stroke.move_to(13.6, 4.0);
            stroke.line_to(13.6, 7.6);
            stroke.quad_to(13.6, 9.0, 15.0, 9.0);
            stroke.line_to(18.6, 9.0);
            match ico {
                PagePlus => {
                    push_polyline(&mut stroke, &[(9.2, 14.2), (14.8, 14.2)]);
                    push_polyline(&mut stroke, &[(12.0, 11.4), (12.0, 17.0)]);
                }
                Pdf => {
                    push_polyline(&mut stroke, &[(8.5, 12.6), (15.5, 12.6)]);
                    push_polyline(&mut stroke, &[(8.5, 16.2), (13.8, 16.2)]);
                }
                _ => {}
            }
        }
        Table => {
            rounded_rect(&mut stroke, 4.0, 4.5, 16.0, 15.0, 2.4);
            // Distinct header divider + row and column lines.
            push_polyline(&mut stroke, &[(4.0, 9.5), (20.0, 9.5)]);
            push_polyline(&mut stroke, &[(4.0, 14.5), (20.0, 14.5)]);
            push_polyline(&mut stroke, &[(9.4, 9.5), (9.4, 19.5)]);
            push_polyline(&mut stroke, &[(14.6, 9.5), (14.6, 19.5)]);
        }
        MindMap => {
            stroke.push_circle(6.6, 12.0, 2.7);
            for y in [5.5, 12.0, 18.5] {
                stroke.push_circle(18.2, y, 2.0);
            }
            // Smooth cubic S-curves branching from the root node.
            stroke.move_to(9.3, 11.2);
            stroke.cubic_to(12.6, 11.2, 12.8, 5.5, 16.2, 5.5);
            push_polyline(&mut stroke, &[(9.3, 12.0), (16.2, 12.0)]);
            stroke.move_to(9.3, 12.8);
            stroke.cubic_to(12.6, 12.8, 12.8, 18.5, 16.2, 18.5);
        }
        Formula => {
            push_polyline(
                &mut stroke,
                &[
                    (3.6, 12.6),
                    (5.6, 12.6),
                    (8.2, 18.8),
                    (11.8, 5.4),
                    (20.4, 5.4),
                ],
            );
            push_polyline(&mut stroke, &[(14.2, 11.4), (19.6, 16.8)]);
            push_polyline(&mut stroke, &[(19.6, 11.4), (14.2, 16.8)]);
        }
        Graph => {
            push_polyline(&mut stroke, &[(4.4, 4.4), (4.4, 19.6), (19.6, 19.6)]);
            // Smooth curve rising across the plot.
            stroke.move_to(7.4, 15.6);
            stroke.cubic_to(9.8, 15.6, 10.6, 9.2, 13.2, 11.2);
            stroke.cubic_to(15.2, 12.8, 16.6, 7.6, 19.0, 6.6);
            fill.push_circle(19.0, 6.6, 1.5);
        }
        Image => {
            rounded_rect(&mut stroke, 3.8, 4.5, 16.4, 15.0, 2.5);
            fill.push_circle(8.4, 9.0, 1.6);
            stroke.move_to(4.4, 17.2);
            stroke.line_to(9.2, 12.6);
            stroke.line_to(12.2, 15.4);
            stroke.line_to(15.4, 11.6);
            stroke.line_to(19.6, 16.6);
        }
        Palette => {
            stroke.move_to(12.0, 3.8);
            stroke.cubic_to(6.8, 3.8, 3.6, 7.4, 3.6, 12.0);
            stroke.cubic_to(3.6, 16.8, 7.4, 20.2, 12.0, 20.2);
            stroke.cubic_to(13.9, 20.2, 14.9, 18.7, 14.0, 17.0);
            stroke.cubic_to(13.2, 15.4, 14.2, 14.0, 16.0, 14.0);
            stroke.line_to(17.4, 14.0);
            stroke.cubic_to(19.5, 14.0, 20.4, 12.5, 20.4, 10.6);
            stroke.cubic_to(20.4, 6.8, 16.8, 3.8, 12.0, 3.8);
            stroke.close();
            // Filled paint wells read crisply at every size.
            for (x, y) in [(7.5, 12.4), (9.4, 8.2), (14.4, 7.8), (17.2, 10.6)] {
                fill.push_circle(x, y, 1.35);
            }
        }
        Dock => {
            rounded_rect(&mut stroke, 3.8, 5.0, 16.4, 14.0, 2.6);
            push_polyline(&mut stroke, &[(8.8, 5.0), (8.8, 19.0)]);
            // Subtle tool indicators inside the dock rail.
            for y in [8.8, 12.0, 15.2] {
                fill.push_circle(6.3, y, 0.95);
            }
        }
        Lock => {
            rounded_rect(&mut stroke, 5.2, 10.8, 13.6, 9.0, 2.4);
            stroke.move_to(8.2, 10.8);
            stroke.line_to(8.2, 8.2);
            stroke.cubic_to(8.2, 5.9, 9.9, 4.2, 12.0, 4.2);
            stroke.cubic_to(14.1, 4.2, 15.8, 5.9, 15.8, 8.2);
            stroke.line_to(15.8, 10.8);
            fill.push_circle(12.0, 15.3, 1.4);
        }
        People => {
            stroke.push_circle(9.0, 7.8, 3.0);
            stroke.move_to(3.8, 19.5);
            stroke.cubic_to(3.8, 16.2, 6.1, 14.0, 9.0, 14.0);
            stroke.cubic_to(11.9, 14.0, 14.2, 16.2, 14.2, 19.5);
            stroke.push_circle(16.6, 8.4, 2.3);
            stroke.move_to(16.8, 14.0);
            stroke.cubic_to(19.0, 14.3, 20.4, 16.0, 20.4, 18.6);
        }
        Globe => {
            stroke.push_circle(12.0, 12.0, 8.4);
            if let Some(oval) = tiny_skia::Rect::from_xywh(8.2, 3.6, 7.6, 16.8) {
                stroke.push_oval(oval);
            }
            push_polyline(&mut stroke, &[(3.8, 12.0), (20.2, 12.0)]);
        }
        Link => {
            // 45-degree interlocking chain links with center bridge.
            stroke.move_to(10.0, 14.0);
            stroke.line_to(14.0, 10.0);
            // Upper-right link arc.
            stroke.move_to(10.8, 8.2);
            stroke.line_to(13.2, 5.8);
            stroke.cubic_to(14.8, 4.2, 17.4, 4.2, 19.0, 5.8);
            stroke.cubic_to(20.6, 7.4, 20.6, 10.0, 19.0, 11.6);
            stroke.line_to(16.6, 14.0);
            // Lower-left link arc.
            stroke.move_to(13.2, 15.8);
            stroke.line_to(10.8, 18.2);
            stroke.cubic_to(9.2, 19.8, 6.6, 19.8, 5.0, 18.2);
            stroke.cubic_to(3.4, 16.6, 3.4, 14.0, 5.0, 12.4);
            stroke.line_to(7.4, 10.0);
        }
        Exit => {
            stroke.move_to(10.2, 4.4);
            stroke.line_to(6.4, 4.4);
            stroke.quad_to(4.6, 4.4, 4.6, 6.2);
            stroke.line_to(4.6, 17.8);
            stroke.quad_to(4.6, 19.6, 6.4, 19.6);
            stroke.line_to(10.2, 19.6);
            push_polyline(&mut stroke, &[(9.5, 12.0), (19.6, 12.0)]);
            push_polyline(&mut stroke, &[(15.8, 8.2), (19.6, 12.0), (15.8, 15.8)]);
        }
    }
    (stroke.finish(), fill.finish())
}
