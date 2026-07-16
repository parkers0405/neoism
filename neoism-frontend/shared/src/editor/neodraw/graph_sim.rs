//! Live, animated note-graph simulation — the Obsidian-style view.
//!
//! A live force simulation (not a static snapshot):
//! this steps a force-directed simulation one tick per frame so the graph
//! visibly settles, nodes can be grabbed and flung, and clicking a node
//! opens its note. It rides on the neodraw [`Camera`](super::Camera) for
//! pan/zoom, so the canvas controls are identical to the sketch editor.

use super::scene::Vec2;

/// A note in the graph.
#[derive(Debug, Clone)]
pub struct GraphNode {
    pub pos: Vec2,
    pub vel: Vec2,
    pub radius: f32,
    pub label: String,
    /// Note path — used to open the note on click.
    pub path: String,
    /// Held in place (while dragged); the sim won't move it.
    pub pinned: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GraphSim {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<(usize, usize)>,
    /// Node currently being dragged, if any.
    pub dragging: Option<usize>,
    /// Residual kinetic energy from the last step; the renderer keeps
    /// animating while this stays above a small threshold.
    pub energy: f32,
    /// Annealing temperature (1 → hot, 0 → frozen). Caps per-step motion
    /// so the layout settles instead of jittering forever; reset on drag.
    temp: f32,
}

/// Below this total speed the layout is considered settled.
const SETTLE_ENERGY: f32 = 0.4;
const DAMPING: f32 = 0.82;

impl GraphSim {
    /// Build a simulation from labels, note paths, and undirected edges.
    /// Nodes are seeded on a ring so the first frames fan out cleanly.
    pub fn new(labels: &[String], paths: &[String], edges: &[(usize, usize)]) -> Self {
        use std::f32::consts::TAU;
        let n = labels.len();
        let mut degree = vec![0usize; n];
        for &(a, b) in edges {
            if a < n {
                degree[a] += 1;
            }
            if b < n {
                degree[b] += 1;
            }
        }
        let nodes = (0..n)
            .map(|i| {
                let t = i as f32 / n.max(1) as f32 * TAU;
                let jitter = (hash01(i as u32) - 0.5) * 40.0;
                GraphNode {
                    pos: Vec2::new(t.cos() * 150.0 + jitter, t.sin() * 150.0 - jitter),
                    vel: Vec2::ZERO,
                    radius: (9.0 + (degree[i] as f32).sqrt() * 5.0).min(44.0),
                    label: labels[i].clone(),
                    path: paths.get(i).cloned().unwrap_or_default(),
                    pinned: false,
                }
            })
            .collect();
        Self {
            nodes,
            edges: edges.to_vec(),
            dragging: None,
            energy: f32::MAX,
            temp: 1.0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Whether the layout is still moving enough to warrant a redraw.
    pub fn is_animating(&self) -> bool {
        self.energy > SETTLE_ENERGY || self.dragging.is_some()
    }

    /// Advance the simulation one tick. Returns whether it's still moving.
    pub fn step(&mut self) -> bool {
        let n = self.nodes.len();
        if n == 0 {
            self.energy = 0.0;
            return false;
        }
        // Fixed ideal edge length — connected nodes settle ~k apart.
        let k = 120.0f32;
        // Beyond this, nodes stop repelling so unconnected clusters don't
        // drift to infinity (gravity then pulls them back together).
        let repel_cutoff = 360.0f32;

        let mut fx = vec![0.0f32; n];
        let mut fy = vec![0.0f32; n];

        // Repulsion between nearby pairs (Coulomb-ish, softened + cut off).
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = self.nodes[i].pos.x - self.nodes[j].pos.x;
                let dy = self.nodes[i].pos.y - self.nodes[j].pos.y;
                let d2 = (dx * dx + dy * dy).max(0.01);
                let dist = d2.sqrt();
                if dist > repel_cutoff {
                    continue;
                }
                let f = (k * k / dist).min(2000.0);
                let (ux, uy) = (dx / dist, dy / dist);
                fx[i] += ux * f;
                fy[i] += uy * f;
                fx[j] -= ux * f;
                fy[j] -= uy * f;
            }
        }
        // Spring attraction along edges.
        for &(a, b) in &self.edges {
            if a >= n || b >= n || a == b {
                continue;
            }
            let dx = self.nodes[a].pos.x - self.nodes[b].pos.x;
            let dy = self.nodes[a].pos.y - self.nodes[b].pos.y;
            let dist = (dx * dx + dy * dy).sqrt().max(0.01);
            let f = dist * dist / k;
            let (ux, uy) = (dx / dist, dy / dist);
            fx[a] -= ux * f;
            fy[a] -= uy * f;
            fx[b] += ux * f;
            fy[b] += uy * f;
        }
        // Gravity toward the origin so disconnected nodes/clusters stay
        // grouped instead of drifting apart.
        for i in 0..n {
            fx[i] -= self.nodes[i].pos.x * 0.03;
            fy[i] -= self.nodes[i].pos.y * 0.03;
        }

        // Cool down so the layout freezes instead of jittering forever.
        self.temp = (self.temp * 0.985).max(0.0);
        let max_step = 30.0 * self.temp;

        // Integrate (skip pinned/dragged nodes).
        let mut energy = 0.0;
        for i in 0..n {
            if self.nodes[i].pinned {
                self.nodes[i].vel = Vec2::ZERO;
                continue;
            }
            let node = &mut self.nodes[i];
            node.vel.x = (node.vel.x + fx[i] * 0.02) * DAMPING;
            node.vel.y = (node.vel.y + fy[i] * 0.02) * DAMPING;
            node.vel.x = node.vel.x.clamp(-max_step, max_step);
            node.vel.y = node.vel.y.clamp(-max_step, max_step);
            node.pos.x += node.vel.x;
            node.pos.y += node.vel.y;
            energy += node.vel.x.abs() + node.vel.y.abs();
        }
        self.energy = energy;
        self.is_animating()
    }

    /// Node whose disc contains world-space `p` (topmost first).
    pub fn node_at(&self, p: Vec2) -> Option<usize> {
        self.nodes
            .iter()
            .enumerate()
            .rev()
            .find(|(_, node)| {
                let dx = node.pos.x - p.x;
                let dy = node.pos.y - p.y;
                dx * dx + dy * dy <= (node.radius + 4.0) * (node.radius + 4.0)
            })
            .map(|(i, _)| i)
    }

    /// Start dragging a node (pins it and re-energizes the sim).
    pub fn begin_drag(&mut self, idx: usize) {
        if let Some(node) = self.nodes.get_mut(idx) {
            node.pinned = true;
            node.vel = Vec2::ZERO;
        }
        self.dragging = Some(idx);
        self.energy = f32::MAX;
        self.temp = self.temp.max(0.6);
    }

    pub fn drag_to(&mut self, p: Vec2) {
        if let Some(idx) = self.dragging {
            if let Some(node) = self.nodes.get_mut(idx) {
                node.pos = p;
            }
            self.energy = f32::MAX;
            self.temp = self.temp.max(0.6);
        }
    }

    /// Release a drag; returns the dropped node so the caller can decide
    /// whether it was a click (open note) or a move.
    pub fn end_drag(&mut self) -> Option<usize> {
        let idx = self.dragging.take()?;
        if let Some(node) = self.nodes.get_mut(idx) {
            node.pinned = false;
        }
        Some(idx)
    }
}

fn hash01(seed: u32) -> f32 {
    let mut z = seed.wrapping_mul(0x9E37_79B1).wrapping_add(0x7F4A_7C15);
    z ^= z >> 16;
    z = z.wrapping_mul(0x85EB_CA6B);
    z ^= z >> 13;
    (z as f32) / (u32::MAX as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("n{i}")).collect()
    }

    #[test]
    fn settles_over_time() {
        let l = labels(10);
        let edges = vec![(0, 1), (1, 2), (2, 3), (3, 4), (0, 5)];
        let mut sim = GraphSim::new(&l, &l, &edges);
        for _ in 0..600 {
            sim.step();
        }
        assert!(
            !sim.is_animating(),
            "sim should settle: energy={}",
            sim.energy
        );
        assert!(sim
            .nodes
            .iter()
            .all(|n| n.pos.x.is_finite() && n.pos.y.is_finite()));
    }

    #[test]
    fn drag_pins_and_reenergizes() {
        let l = labels(4);
        let mut sim = GraphSim::new(&l, &l, &[(0, 1)]);
        sim.begin_drag(0);
        assert!(sim.nodes[0].pinned);
        sim.drag_to(Vec2::new(123.0, -50.0));
        assert_eq!(sim.nodes[0].pos, Vec2::new(123.0, -50.0));
        sim.step(); // pinned node must not move
        assert_eq!(sim.nodes[0].pos, Vec2::new(123.0, -50.0));
        assert_eq!(sim.end_drag(), Some(0));
        assert!(!sim.nodes[0].pinned);
    }

    #[test]
    fn node_hit_test() {
        let l = labels(2);
        let mut sim = GraphSim::new(&l, &l, &[]);
        sim.nodes[0].pos = Vec2::new(0.0, 0.0);
        sim.nodes[0].radius = 20.0;
        sim.nodes[1].pos = Vec2::new(500.0, 500.0);
        assert_eq!(sim.node_at(Vec2::new(5.0, 5.0)), Some(0));
        assert_eq!(sim.node_at(Vec2::new(250.0, 250.0)), None);
    }
}
