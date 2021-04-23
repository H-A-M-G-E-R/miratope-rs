use std::{collections::HashMap, hash::Hash};

use bevy::{
    prelude::Mesh,
    render::{mesh::Indices, pipeline::PrimitiveTopology},
};

use lyon::math::point;
use lyon::path::Path;
use lyon::tessellation::*;

use crate::polytope::{geometry::Point, Concrete, ElementList};

#[derive(Clone, Copy)]
/// Represents a set of at most two elements.
pub enum Pair<T> {
    None,
    One(T),
    Two(T, T),
}

impl<T: Copy> Pair<T> {
    /// Pushes a value onto the pair, panics if it doesn't fit.
    pub fn push(&mut self, value: T) {
        *self = match self {
            Self::None => Self::One(value),
            Self::One(first) => Self::Two(*first, value),
            Self::Two(_, _) => panic!("oops"),
        };
    }
}

/// A loop of vertices. Each vertex is mapped to indices in an edge vector,
/// which stores the two other vertices it's connected to.
pub struct VertexLoop<T> {
    /// A map from vertices to indices.
    vertex_map: HashMap<T, usize>,

    /// A map from indices to pairs of vertices.
    edges: Vec<Pair<T>>,
}

impl<T: Copy + Default + Eq + Hash> VertexLoop<T> {
    /// Initializes a new, empty vertex loop.
    pub fn new() -> Self {
        Self {
            vertex_map: HashMap::new(),
            edges: Vec::new(),
        }
    }

    /// Initializes a vertex loop with a given capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            vertex_map: HashMap::new(),
            edges: Vec::with_capacity(capacity),
        }
    }

    /// Gets the index of a vertex, or adds it if it doesn't exist.
    fn index_mut(&mut self, vertex: T) -> usize {
        use std::collections::hash_map::Entry;

        let len = self.vertex_map.len();
        let entry = self.vertex_map.entry(vertex);

        match entry {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                entry.insert(len);
                self.edges.push(Pair::None);
                len
            }
        }
    }

    /// Gets the index of a vertex, or returns `None` if it doesn't exist.
    fn index(&self, vertex: T) -> Option<usize> {
        self.vertex_map.get(&vertex).copied()
    }

    /// Pushes a pair of vertices into the vertex loop.
    pub fn push(&mut self, vertex0: T, vertex1: T) {
        let idx0 = self.index_mut(vertex0);
        let idx1 = self.index_mut(vertex1);

        self.edges[idx0].push(vertex1);
        self.edges[idx1].push(vertex0);
    }

    pub fn edge(&self, idx: usize) -> Option<(T, T)> {
        if let Pair::Two(v0, v1) = self.edges.get(idx)? {
            Some((*v0, *v1))
        } else {
            None
        }
    }

    /// Returns the number of edges in the vertex loop.
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    /// Cycles through the vertex loop, returns the vector of vertices in cyclic
    /// order.
    pub fn cycle(&self) -> Option<Vec<T>> {
        let mut cycle = Vec::with_capacity(self.len());

        let mut prev_idx = 0;
        let (v, _) = self.edge(0)?;
        cycle.push(v);
        let mut idx = self.index(v).unwrap();

        // We get the vertices from our current index,
        loop {
            let (v0, v1) = self.edge(idx)?;
            let idx0 = self.index(v0).unwrap();
            let idx1 = self.index(v1).unwrap();

            idx = if idx0 == prev_idx {
                prev_idx = idx;
                cycle.push(v1);
                idx1
            } else {
                prev_idx = idx;
                cycle.push(v0);
                idx0
            };

            if idx == 0 {
                break;
            }
        }

        if cycle.len() == self.len() {
            Some(cycle)
        } else {
            None
        }
    }
}

/// A [`Concrete`], together with a triangulation used to render it.
///
/// This struct doesn't actually implement [`Polytope`](crate::Polytope), though
/// it still acts as a type of polytope by virtue of storing one directly.
#[derive(Debug, Clone)]
pub struct Renderable {
    /// The underlying concrete polytope.
    pub concrete: Concrete,

    /// Extra vertices that might be needed for the triangulation.
    extra_vertices: Vec<Point>,

    /// Indices of the vertices that make up the triangles.
    triangles: Vec<VertexIndex>,
}

#[derive(Clone, Copy, Debug)]
enum VertexIndex {
    Concrete(usize),
    Extra(usize),
}

impl Renderable {
    /// Generates the triangulation of a `Concrete`.
    pub fn new(concrete: Concrete) -> Self {
        let mut extra_vertices = Vec::new();
        let mut triangles = Vec::new();

        let empty_els = ElementList::new();
        let edges = concrete.abs.ranks.get(1).unwrap_or(&empty_els);
        let faces = concrete.abs.ranks.get(2).unwrap_or(&empty_els);

        // We render each face separately.
        for face in faces.iter() {
            let mut vertex_loop = VertexLoop::with_capacity(face.subs.len());

            // We first figure out the vertices in order.
            for [v0, v1] in face.subs.iter().map(|&i| {
                let edge = &edges[i];
                let len = edge.subs.len();
                assert_eq!(len, 2, "Edge has {} subelements, expected 2.", len);
                [edge.subs[0], edge.subs[1]]
            }) {
                vertex_loop.push(v0, v1);
            }

            // We cycle through the vertices of the polygon in order.
            let cycle = vertex_loop.cycle().unwrap();
            let mut cycle_iter = cycle.iter();

            // We build a path from the polygon.
            let mut builder = Path::builder();
            let p = &concrete.vertices[*cycle_iter.next().unwrap()];
            builder.begin(point(p[0] as f32, p[1] as f32));
            for &idx in cycle_iter {
                let p = &concrete.vertices[idx];
                builder.line_to(point(p[0] as f32, p[1] as f32));
            }
            builder.close();

            // We tesselate this path.
            let path = builder.build();
            let mut geometry: VertexBuffers<_, u16> = VertexBuffers::new();
            FillTessellator::new()
                .tessellate_with_ids(
                    path.id_iter(),
                    &path,
                    None,
                    &FillOptions::with_fill_rule(FillOptions::default(), FillRule::EvenOdd),
                    &mut BuffersBuilder::new(&mut geometry, |vertex: FillVertex| {
                        vertex.sources().next().unwrap()
                    }),
                )
                .unwrap();

            // Renders only the last face for now (easily fixable, I'll do it later)

            // We map the output vertices to the original ones, and add any
            // extra vertices that may be needed.

            let mut vertex_hash = HashMap::new();

            for (new_id, vertex_source) in geometry.vertices.into_iter().enumerate() {
                match vertex_source {
                    VertexSource::Endpoint { id } => {
                        vertex_hash.insert(new_id, VertexIndex::Concrete(cycle[id.to_usize()]));
                    }
                    VertexSource::Edge { from, to, t } => {
                        let t = t as f64;

                        let v0 = &concrete.vertices[from.to_usize()];
                        let v1 = &concrete.vertices[to.to_usize()];
                        let p = v1 * t + v0 * (1.0 - t);

                        vertex_hash.insert(new_id, VertexIndex::Extra(extra_vertices.len()));
                        extra_vertices.push(p);
                    }
                }
            }

            triangles.append(
                &mut geometry
                    .indices
                    .into_iter()
                    .map(|idx| *vertex_hash.get(&(idx as usize)).unwrap())
                    .collect(),
            );
        }

        Renderable {
            concrete,
            extra_vertices,
            triangles,
        }
    }

    /// Gets the coordinates of the vertices, after projecting down into 3D.
    fn get_vertex_coords(&self) -> Vec<[f32; 3]> {
        // Enables orthogonal projection.
        const ORTHOGONAL: bool = false;

        let vert_iter = self
            .concrete
            .vertices
            .iter()
            .chain(self.extra_vertices.iter());

        // If the polytope is at most 3D, we just embed it into 3D space.
        if ORTHOGONAL || self.concrete.dim().unwrap_or(0) <= 3 {
            vert_iter
                .map(|point| {
                    let mut iter = point.iter().copied().take(3);
                    let x = iter.next().unwrap_or(0.0);
                    let y = iter.next().unwrap_or(0.0);
                    let z = iter.next().unwrap_or(0.0);
                    [x as f32, y as f32, z as f32]
                })
                .collect()
        }
        // Else, we project it down.
        else {
            // Distance from the projection planes.
            const DIST: f64 = 2.0;

            vert_iter
                .map(|point| {
                    let factor: f64 = point.iter().skip(3).map(|x| x + DIST).product();

                    // We scale the first three coordinates accordingly.
                    let mut iter = point.iter().copied().take(3);
                    let x: f64 = iter.next().unwrap() / factor;
                    let y: f64 = iter.next().unwrap() / factor;
                    let z: f64 = iter.next().unwrap() / factor;
                    [x as f32, y as f32, z as f32]
                })
                .collect()
        }
    }

    fn parse_index(&self, idx: VertexIndex) -> u16 {
        let concrete_len = self.concrete.vertices.len();

        (match idx {
            VertexIndex::Concrete(i) => i,
            VertexIndex::Extra(i) => i + concrete_len,
        }) as u16
    }

    /// Generates a mesh from the polytope.
    pub fn get_mesh(&self) -> Mesh {
        use itertools::Itertools;

        let vertices = self.get_vertex_coords();
        let mut indices = Vec::with_capacity(self.triangles.len() * 3);
        for mut chunk in &self.triangles.iter().chunks(3) {
            for _ in 0..3 {
                indices.push(self.parse_index(*chunk.next().unwrap()));
            }
        }

        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList);
        mesh.set_attribute(
            Mesh::ATTRIBUTE_NORMAL,
            vec![[0.0, 1.0, 0.0]; vertices.len()],
        );
        mesh.set_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; vertices.len()]);
        mesh.set_attribute(Mesh::ATTRIBUTE_POSITION, vertices);
        mesh.set_indices(Some(Indices::U16(indices)));

        mesh
    }

    /// Generates the wireframe for a polytope.
    pub fn get_wireframe(&self) -> Mesh {
        let empty_els = ElementList::new();
        let edges = self.concrete.abs.ranks.get(1).unwrap_or(&empty_els);
        let vertices = self.get_vertex_coords();
        let mut indices = Vec::with_capacity(edges.len() * 2);

        for edge in edges.iter() {
            indices.push(edge.subs[0] as u16);
            indices.push(edge.subs[1] as u16);
        }

        let mut mesh = Mesh::new(PrimitiveTopology::LineList);
        mesh.set_attribute(
            Mesh::ATTRIBUTE_NORMAL,
            vec![[0.0, 1.0, 0.0]; vertices.len()],
        );
        mesh.set_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; vertices.len()]);
        mesh.set_attribute(Mesh::ATTRIBUTE_POSITION, vertices);
        mesh.set_indices(Some(Indices::U16(indices)));

        mesh
    }
}
