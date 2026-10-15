//! Dig F5 — Wild official region graph (1D chain).
//!
//! Fixed adjacency replaces LAN UDP discover. Each host loads the same
//! graph file + its `--seam-region-id` and wires listen / dial / allowlist.
//! See [`docs/HALO_SHARD.md`].

use std::collections::HashMap;
use std::fmt;

use wk_voxel::SeamSide;

use crate::seam_discover::SeamAllowlist;

/// Parsed official graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionGraph {
    pub mesh_id: String,
    /// region_id → TCP seam advertise / listen address.
    pub nodes: HashMap<String, String>,
    /// Ordered 1D edges: `(west_region, east_region)`.
    pub edges: Vec<(String, String)>,
}

/// How this host should wire seams from the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionSeamPlan {
    pub mesh_id: String,
    pub region_id: String,
    pub open_west: bool,
    pub open_east: bool,
    pub allowlist: SeamAllowlist,
    /// Listen when we are the geographic east endpoint of an edge (West face open).
    pub listen: Option<String>,
    /// Dial peers: `(our_side, peer_tcp_addr)`. East face dials West neighbor (F4).
    pub dials: Vec<(SeamSide, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    Empty,
    MissingMesh,
    DupNode(String),
    UnknownNode(String),
    DupEdge(String, String),
    SelfEdge(String),
    MissingRegion(String),
    Parse(String),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::Empty => write!(f, "empty seam graph"),
            GraphError::MissingMesh => write!(f, "seam graph missing `mesh` line"),
            GraphError::DupNode(id) => write!(f, "duplicate node {id:?}"),
            GraphError::UnknownNode(id) => write!(f, "edge references unknown node {id:?}"),
            GraphError::DupEdge(a, b) => write!(f, "duplicate edge {a}->{b}"),
            GraphError::SelfEdge(id) => write!(f, "self-edge on {id:?}"),
            GraphError::MissingRegion(id) => write!(f, "region {id:?} not in graph"),
            GraphError::Parse(s) => write!(f, "seam graph parse: {s}"),
        }
    }
}

impl std::error::Error for GraphError {}

impl RegionGraph {
    /// Parse Dig F5 graph text.
    ///
    /// ```text
    /// mesh official-1
    /// node alpha 127.0.0.1:17421
    /// node beta  127.0.0.1:17422
    /// edge alpha beta
    /// ```
    pub fn parse(text: &str) -> Result<Self, GraphError> {
        let mut mesh_id = String::new();
        let mut nodes = HashMap::new();
        let mut edges = Vec::new();
        for (lineno, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let kind = parts.next().unwrap_or("");
            match kind {
                "mesh" => {
                    let id = parts.next().ok_or_else(|| {
                        GraphError::Parse(format!("line {}: mesh needs id", lineno + 1))
                    })?;
                    if parts.next().is_some() {
                        return Err(GraphError::Parse(format!(
                            "line {}: mesh takes one id",
                            lineno + 1
                        )));
                    }
                    mesh_id = id.to_string();
                }
                "node" => {
                    let id = parts.next().ok_or_else(|| {
                        GraphError::Parse(format!("line {}: node needs id", lineno + 1))
                    })?;
                    let addr = parts.next().ok_or_else(|| {
                        GraphError::Parse(format!("line {}: node needs host:port", lineno + 1))
                    })?;
                    if parts.next().is_some() {
                        return Err(GraphError::Parse(format!(
                            "line {}: node takes id host:port",
                            lineno + 1
                        )));
                    }
                    if nodes.insert(id.to_string(), addr.to_string()).is_some() {
                        return Err(GraphError::DupNode(id.to_string()));
                    }
                }
                "edge" => {
                    let west = parts.next().ok_or_else(|| {
                        GraphError::Parse(format!("line {}: edge needs west east", lineno + 1))
                    })?;
                    let east = parts.next().ok_or_else(|| {
                        GraphError::Parse(format!("line {}: edge needs west east", lineno + 1))
                    })?;
                    if parts.next().is_some() {
                        return Err(GraphError::Parse(format!(
                            "line {}: edge takes two region ids",
                            lineno + 1
                        )));
                    }
                    if west == east {
                        return Err(GraphError::SelfEdge(west.to_string()));
                    }
                    edges.push((west.to_string(), east.to_string()));
                }
                other => {
                    return Err(GraphError::Parse(format!(
                        "line {}: unknown keyword {other:?}",
                        lineno + 1
                    )));
                }
            }
        }
        if mesh_id.is_empty() {
            return Err(GraphError::MissingMesh);
        }
        if nodes.is_empty() {
            return Err(GraphError::Empty);
        }
        for (w, e) in &edges {
            if !nodes.contains_key(w) {
                return Err(GraphError::UnknownNode(w.clone()));
            }
            if !nodes.contains_key(e) {
                return Err(GraphError::UnknownNode(e.clone()));
            }
        }
        // Unique undirected pairs.
        let mut seen = std::collections::HashSet::new();
        for (w, e) in &edges {
            let key = if w < e {
                (w.clone(), e.clone())
            } else {
                (e.clone(), w.clone())
            };
            if !seen.insert(key) {
                return Err(GraphError::DupEdge(w.clone(), e.clone()));
            }
        }
        Ok(Self {
            mesh_id,
            nodes,
            edges,
        })
    }

    /// Plan listen/dial/allowlist for `region_id`.
    ///
    /// Convention (aligned with Dig F4): geographic **west** opens East face
    /// and **dials**; geographic **east** opens West face and **listens**.
    pub fn plan(&self, region_id: &str) -> Result<RegionSeamPlan, GraphError> {
        if !self.nodes.contains_key(region_id) {
            return Err(GraphError::MissingRegion(region_id.to_string()));
        }
        let mut open_west = false;
        let mut open_east = false;
        let mut allow = SeamAllowlist::new();
        let mut listen = None;
        let mut dials = Vec::new();

        for (west, east) in &self.edges {
            if west == region_id {
                // We are geographic west → open east face, dial east peer.
                open_east = true;
                allow.allow(east.clone());
                let addr = self.nodes.get(east).cloned().unwrap();
                dials.push((SeamSide::East, addr));
            } else if east == region_id {
                // We are geographic east → open west face, listen for west dialer.
                open_west = true;
                allow.allow(west.clone());
                listen = Some(self.nodes.get(region_id).cloned().unwrap());
            }
        }

        Ok(RegionSeamPlan {
            mesh_id: self.mesh_id.clone(),
            region_id: region_id.to_string(),
            open_west,
            open_east,
            allowlist: allow,
            listen,
            dials,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# Dig F5 sample
mesh official-1
node alpha 127.0.0.1:17421
node beta  127.0.0.1:17422
node gamma 127.0.0.1:17423
edge alpha beta
edge beta gamma
"#;

    #[test]
    fn parse_chain() {
        let g = RegionGraph::parse(SAMPLE).unwrap();
        assert_eq!(g.mesh_id, "official-1");
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.edges.len(), 2);
    }

    #[test]
    fn plan_ends() {
        let g = RegionGraph::parse(SAMPLE).unwrap();
        let a = g.plan("alpha").unwrap();
        assert!(a.open_east && !a.open_west);
        assert!(a.listen.is_none());
        assert_eq!(a.dials, vec![(SeamSide::East, "127.0.0.1:17422".into())]);
        assert!(a.allowlist.allows("beta"));
        assert!(!a.allowlist.allows("gamma"));

        let c = g.plan("gamma").unwrap();
        assert!(c.open_west && !c.open_east);
        assert_eq!(c.listen.as_deref(), Some("127.0.0.1:17423"));
        assert!(c.dials.is_empty());
        assert!(c.allowlist.allows("beta"));
    }

    #[test]
    fn plan_middle_both_faces() {
        let g = RegionGraph::parse(SAMPLE).unwrap();
        let b = g.plan("beta").unwrap();
        assert!(b.open_west && b.open_east);
        assert_eq!(b.listen.as_deref(), Some("127.0.0.1:17422"));
        assert_eq!(b.dials, vec![(SeamSide::East, "127.0.0.1:17423".into())]);
        assert!(b.allowlist.allows("alpha"));
        assert!(b.allowlist.allows("gamma"));
    }

    #[test]
    fn rejects_bad_graph() {
        assert!(matches!(
            RegionGraph::parse("node a 1:1\n"),
            Err(GraphError::MissingMesh)
        ));
        assert!(matches!(
            RegionGraph::parse("mesh m\nedge a b\n"),
            Err(GraphError::Empty)
        ));
        let err = RegionGraph::parse(
            "mesh m\nnode a 1:1\nnode b 1:2\nedge a b\nedge b a\n",
        )
        .unwrap_err();
        assert!(matches!(err, GraphError::DupEdge(_, _)));
    }
}
