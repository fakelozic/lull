//! The tray icon: Lull's ring in the Spectrum colours. Drawn once per state and never animated.
//! Playing and paused differ in shape as well as colour: a dot in the ring, or two bars.

use std::f32::consts::TAU;
use std::sync::OnceLock;

use crate::visual::oklch;

/// The sizes trays ask for, 1x to 3x.
const SIZES: [usize; 4] = [22, 32, 44, 66];
/// Mid grey-green: 3:1 or better against both light and dark panels.
const PAUSED: [f32; 3] = [129.0, 139.0, 121.0];

/// The pixmaps for one state, as ARGB32 in network byte order.
pub(crate) fn pixmaps(playing: bool) -> Vec<ksni::Icon> {
    static PAUSED_ICON: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
    static PLAYING_ICON: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
    let slot = if playing { &PLAYING_ICON } else { &PAUSED_ICON };
    slot.get_or_init(|| SIZES.map(|size| draw(size, playing)).to_vec()).clone()
}

fn draw(size: usize, playing: bool) -> ksni::Icon {
    let middle = size as f32 / 2.0;
    let unit = size as f32 / 22.0;
    let mut data = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 + 0.5 - middle, y as f32 + 0.5 - middle);
            let out = dx.hypot(dy);
            let cover = |inside: f32| (inside + 0.5).clamp(0.0, 1.0);
            let ring = cover(1.3 * unit - (out - 8.3 * unit).abs());
            let centre = if playing {
                cover(3.2 * unit - out)
            } else {
                cover(0.9 * unit - (dx.abs() - 2.1 * unit).abs()) * cover(3.4 * unit - dy.abs())
            };
            let alpha = ring.max(centre);
            let colour = if playing { oklch(0.80, 0.13, dx.atan2(-dy).rem_euclid(TAU) + 2.5) } else { PAUSED };
            data.push((alpha * 255.0).round() as u8);
            data.extend(colour.map(|channel| channel.round() as u8));
        }
    }
    ksni::Icon { width: size as i32, height: size as i32, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_differ_in_shape_not_only_colour() {
        let shape = |playing| pixmaps(playing)[0].data.iter().step_by(4).copied().collect::<Vec<u8>>();
        assert_ne!(shape(true), shape(false));
        for icon in pixmaps(true) {
            assert_eq!(icon.data.len(), (icon.width * icon.height * 4) as usize);
            assert_eq!(icon.data[0], 0, "corners are see-through");
        }
    }
}
