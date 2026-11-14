//! Opt-in retained translation/opacity tracks. Layout, glyph placement, presence and app events
//! remain CPU-owned. Callers sample on demand for hit testing and stop ticks at `settle_at()`.
use bytemuck::{Pod, Zeroable};
use rune_motion::Spring;

pub const SHADER: &str = include_str!("springs.wgsl");

pub fn split_time(time: f64) -> [f32; 2] {
    assert!(time.is_finite() && time >= 0.0 && time < 16_777_216.0);
    let hi = time.floor() as f32;
    [hi, (time - hi as f64) as f32]
}

/// GPU ABI, 48 bytes. Keep fields private: construction validates finite inputs.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Track {
    abcd: [f32; 4],
    rates: [f32; 2],
    target: f32,
    kind: u32,
    start: [f32; 2],
    duration: f32,
    pad: f32,
}

impl Track {
    pub fn new(spring: Spring, time: f64, from: f32, target: f32, velocity: f32, epsilon: f32) -> Self {
        assert!([from, target, velocity].iter().all(|v| v.is_finite()));
        let duration = spring.settling_duration_from(from, velocity, target, epsilon);
        let a = from - target;
        let w = spring.omega();
        let z = spring.zeta();
        let (kind, abcd, rates) = if a == 0.0 && velocity == 0.0 {
            (0, [0.0; 4], [0.0; 2])
        } else if (z - 1.0).abs() < 1e-4 {
            let b = velocity + w * a;
            (2, [a, b, b - w * a, -w * b], [w, 0.0])
        } else if z < 1.0 {
            let sigma = z * w;
            let wd = w * (1.0 - z * z).sqrt();
            let b = (velocity + sigma * a) / wd;
            (1, [a, b, b * wd - sigma * a, -a * wd - sigma * b], [sigma, wd])
        } else {
            let root = (z * z - 1.0).sqrt();
            let r1 = -w * (z - root);
            let r2 = -w * (z + root);
            let c1 = (velocity - r2 * a) / (r1 - r2);
            (3, [c1, a - c1, r1, r2], [0.0; 2])
        };
        assert!(abcd.iter().chain(rates.iter()).all(|x| x.is_finite()));
        Self { abcd, rates, target, kind, start: split_time(time), duration, pad: 0.0 }
    }

    pub fn constant(value: f32) -> Self {
        Self::new(Spring::DEFAULT, 0.0, value, value, 0.0, 0.001)
    }

    pub fn settle_at(&self) -> f64 {
        self.start[0] as f64 + self.start[1] as f64 + self.duration as f64
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Node {
    pub x: Track,
    pub y: Track,
    pub opacity: Track,
}

impl Node {
    pub fn settle_at(&self) -> f64 {
        self.x.settle_at().max(self.y.settle_at()).max(self.opacity.settle_at())
    }
}

pub(crate) struct Bindings {
    pub group: wgpu::BindGroup,
    pub nodes: wgpu::Buffer,
    pub deadlines: Vec<f64>,
    pub settle_at: f64,
}

impl Bindings {
    pub fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spring properties"),
            entries: &[0, 1].map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
        })
    }

    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, nodes: &[Node], ids: &[u32]) -> Self {
        use wgpu::util::DeviceExt;
        assert!(!nodes.is_empty() && !ids.is_empty());
        assert!(ids.iter().all(|&id| (id as usize) < nodes.len()));
        let deadlines: Vec<_> = nodes.iter().map(Node::settle_at).collect();
        let settle_at = deadlines.iter().copied().fold(0.0, f64::max);
        let nodes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spring nodes"),
            contents: bytemuck::cast_slice(nodes),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let ids = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spring ids"),
            contents: bytemuck::cast_slice(ids),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spring properties"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: nodes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: ids.as_entire_binding() },
            ],
        });
        Self { group, nodes, deadlines, settle_at }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_retarget_settle_bounds_hold_for_the_tail() {
        assert_eq!(std::mem::size_of::<Track>(), 48);
        assert_eq!(std::mem::size_of::<Node>(), 144);
        for bounce in [-0.9, -0.2, 0.0, 0.16, 0.8] {
            let spring = Spring::new(0.4, bounce);
            for velocity in [-800.0, 0.0, 900.0] {
                let (from, velocity) = spring.sample(-120.0, velocity, 40.0, 0.12);
                let duration = spring.settling_duration_from(from, velocity, -30.0, 0.001);
                for i in 0..200 {
                    let (x, v) = spring.sample(from, velocity, -30.0, duration + i as f32 / 120.0);
                    assert!((x + 30.0).abs() <= 0.00101, "{bounce}: {x}");
                    assert!(v.abs() <= 0.01001, "{bounce}: {v}");
                }
            }
        }
        assert_eq!(Track::constant(1.0).settle_at(), 0.0);
    }
}
