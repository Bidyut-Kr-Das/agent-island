use super::theme::{BG, OK, WARN};
use crate::state::{Finish, Status};
use eframe::egui::{Color32, Painter, Pos2, Shape, Stroke, vec2};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BlobState {
    Idle,
    Working,
    Waiting,
    Done,
}

impl BlobState {
    pub fn of(status: Status, finished: Option<Finish>) -> BlobState {
        match (status, finished) {
            (Status::Waiting(_), _) => BlobState::Waiting,
            (Status::Working, _) => BlobState::Working,
            (Status::Idle, Some(_)) => BlobState::Done,
            (Status::Idle, None) => BlobState::Idle,
        }
    }
}

/// Working squash-bounce at `t` seconds: (dy in 24-unit box, scale x, scale y).
pub fn bounce(t: f64) -> (f32, f32, f32) {
    // Keyframes 0%, 40%, 80%, 100% of 0.9 s, eased per segment like CSS ease-in-out.
    const K: [(f32, f32, f32, f32); 4] = [(0.0, 0.0, 1.0, 1.0), (0.4, -1.6, 0.97, 1.03), (0.8, 0.0, 1.04, 0.95), (1.0, 0.0, 1.0, 1.0)];
    let p = (t / 0.9).rem_euclid(1.0) as f32;
    let i = K.iter().rposition(|k| k.0 <= p).unwrap_or(0).min(2);
    let (a, b) = (K[i], K[i + 1]);
    let e = ease((p - a.0) / (b.0 - a.0));
    (a.1 + (b.1 - a.1) * e, a.2 + (b.2 - a.2) * e, a.3 + (b.3 - a.3) * e)
}

/// Smoothstep: ponytail: close to CSS ease-in-out, exact cubic-bezier(.42,0,.58,1) if anyone can tell.
fn ease(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

type Bez = [(f32, f32); 4];
/// Body `M3.5 17c0-6.5 3.8-11 8.5-11s8.5 4.5 8.5 11c0 2.4-2.6 3.5-8.5 3.5S3.5 19.4 3.5 17z` as absolute cubics.
const BODY: [Bez; 4] = [
    [(3.5, 17.0), (3.5, 10.5), (7.3, 6.0), (12.0, 6.0)],
    [(12.0, 6.0), (16.7, 6.0), (20.5, 10.5), (20.5, 17.0)],
    [(20.5, 17.0), (20.5, 19.4), (17.9, 20.5), (12.0, 20.5)],
    [(12.0, 20.5), (6.1, 20.5), (3.5, 19.4), (3.5, 17.0)],
];

fn cubic(b: &Bez, t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    (0..4).fold((0.0, 0.0), |a, i| (a.0 + w[i] * b[i].0, a.1 + w[i] * b[i].1))
}

/// Paints one blob in a `size` x `size` box whose bottom center is `center_bottom`.
pub fn paint_blob(p: &Painter, center_bottom: Pos2, size: f32, color: Color32, st: BlobState, t: f64) {
    let s = size / 24.0;
    let min = center_bottom - vec2(size / 2.0, size);
    // Idle is drawn at 50% opacity; the blob always sits on BG, so fade toward BG instead of alpha.
    let fade = |c: Color32| if st == BlobState::Idle { BG.lerp_to_gamma(c, 0.5) } else { c };
    let (dy, sx, sy) = if st == BlobState::Working { bounce(t) } else { (0.0, 1.0, 1.0) };
    // Group transform: scale about the body's bottom center (12, 20.5), then translate y.
    let g = |x: f32, y: f32| min + vec2((12.0 + (x - 12.0) * sx) * s, (20.5 + (y - 20.5) * sy + dy) * s);
    let at = |x: f32, y: f32| min + vec2(x * s, y * s);
    let dark = fade(BG);

    let body: Vec<Pos2> = BODY.iter().flat_map(|b| (0..12).map(move |i| cubic(b, i as f32 / 12.0))).map(|(x, y)| g(x, y)).collect();
    p.add(Shape::convex_polygon(body, fade(color), Stroke::NONE));
    p.add(Shape::ellipse_filled(g(8.4, 10.2), vec2(2.0 * sx, 1.2 * sy) * s, Color32::from_white_alpha(if st == BlobState::Idle { 44 } else { 89 })));
    let eyes = |y: f32, r: f32| {
        for x in [9.3, 14.7] {
            p.add(Shape::ellipse_filled(g(x, y), vec2(r * sx, r * 1.3 * sy) * s, dark));
        }
    };
    let line = |pts: Vec<Pos2>, w: f32| {
        let st = Stroke::new(w * s, dark);
        for e in [pts[0], pts[pts.len() - 1]] {
            p.circle_filled(e, w * s / 2.0, dark); // round caps
        }
        p.add(Shape::line(pts, st));
    };
    match st {
        BlobState::Done => {
            for x0 in [8.0, 13.4] {
                let q = [(x0, 14.6), (x0 + 1.3, 12.8), (x0 + 2.6, 14.6)];
                let pts = (0..=8).map(|i| {
                    let (t, u) = (i as f32 / 8.0, 1.0 - i as f32 / 8.0);
                    g(u * u * q[0].0 + 2.0 * u * t * q[1].0 + t * t * q[2].0, u * u * q[0].1 + 2.0 * u * t * q[1].1 + t * t * q[2].1)
                });
                line(pts.collect(), 1.3);
            }
        }
        BlobState::Waiting => {
            eyes(12.6, 1.1);
            p.circle_filled(g(12.0, 16.8), 0.8 * s * sx.min(sy), dark);
        }
        _ => eyes(14.0, 1.15),
    }
    let badge = |fill: Color32, k: f32| {
        p.circle_filled(at(20.5, 4.5), 3.4 * k * s, fill);
        p.circle_stroke(at(20.5, 4.5), 3.4 * k * s, Stroke::new(s, BG));
    };
    match st {
        BlobState::Waiting => {
            // Pulse 1 -> 1.15 -> 1 over 1.2 s, scaled about the badge center.
            let ph = (t / 1.2).rem_euclid(1.0) as f32;
            let k = 1.0 + 0.15 * ease(1.0 - (2.0 * ph - 1.0).abs());
            badge(WARN, k);
            let b = |x: f32, y: f32| at(20.5 + (x - 20.5) * k, 4.5 + (y - 4.5) * k);
            line(vec![b(20.5, 3.0), b(20.5, 4.6)], k); // the "!" bar: 1 wide, y 2.5..5.1 with round ends
            p.circle_filled(b(20.5, 6.4), 0.6 * k * s, BG);
        }
        BlobState::Done => {
            badge(OK, 1.0);
            line(vec![at(18.9, 4.6), at(20.0, 5.7), at(22.1, 3.4)], 1.1);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::WaitKind;

    fn close(a: (f32, f32, f32), b: (f32, f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4 && (a.2 - b.2).abs() < 1e-4
    }

    #[test]
    fn squash_offsets_match_keyframes() {
        assert!(close(bounce(0.0), (0.0, 1.0, 1.0)));
        assert!(close(bounce(0.36), (-1.6, 0.97, 1.03)));
        assert!(close(bounce(0.72), (0.0, 1.04, 0.95)));
        assert!(close(bounce(0.9), (0.0, 1.0, 1.0)), "loops every 0.9 s");
        assert!(close(bounce(0.9 + 0.36), (-1.6, 0.97, 1.03)));
    }

    #[test]
    fn bounce_eases_between_keyframes() {
        let (dy, _, _) = bounce(0.18);
        assert!(dy < -0.1 && dy > -1.5, "midway between 0 and -1.6, got {dy}");
    }

    #[test]
    fn blob_state_from_session() {
        assert_eq!(BlobState::of(Status::Waiting(WaitKind::Question), None), BlobState::Waiting);
        assert_eq!(BlobState::of(Status::Waiting(WaitKind::Permission), Some(Finish::Ok)), BlobState::Waiting);
        assert_eq!(BlobState::of(Status::Working, None), BlobState::Working);
        assert_eq!(BlobState::of(Status::Idle, Some(Finish::Error)), BlobState::Done);
        assert_eq!(BlobState::of(Status::Idle, None), BlobState::Idle);
    }
}
