//! Strip studio-only pixels into a world-ready body payload.

use serde::{Deserialize, Serialize};

use crate::neural::StudioNet;
use crate::tissue::{StudioBody, TissuePaint};

pub const BODY_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedBody {
    pub schema: u32,
    pub paint: TissuePaint,
    /// Frozen controller weights (optional).
    pub net: Option<StudioNet>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    EmptyBody,
}

/// Drop fixture / force-sensor / empty; keep morphology + joints + nerve.
pub fn export_body(body: &StudioBody) -> Result<ExportedBody, ExportError> {
    export_body_with_net(body, None)
}

pub fn export_body_with_net(
    body: &StudioBody,
    net: Option<StudioNet>,
) -> Result<ExportedBody, ExportError> {
    let mut out = TissuePaint::new(body.paint.width, body.paint.height);
    let mut any = false;
    for y in 0..body.paint.height {
        for x in 0..body.paint.width {
            let k = body.paint.get(x, y);
            if k.exportable() {
                out.set(x, y, k);
                any = true;
            }
        }
    }
    if !any {
        return Err(ExportError::EmptyBody);
    }
    Ok(ExportedBody {
        schema: BODY_SCHEMA_VERSION,
        paint: out,
        net,
    })
}

/// Postcard bytes for `.gvsebody` files.
pub fn encode_body(exp: &ExportedBody) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(exp)
}

pub fn decode_body(bytes: &[u8]) -> Result<ExportedBody, postcard::Error> {
    postcard::from_bytes(bytes)
}

/// Load an exported morphology back into a studio body (world spawn
/// wires the same paint into `wk-voxel-app` later).
pub fn import_body_paint(body: &mut StudioBody, exp: &ExportedBody) {
    body.paint = exp.paint.clone();
    body.activated = false;
    body.graph = None;
    body.net = exp.net.clone();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::neural::StudioNet;
    use crate::tissue::TissueKind;

    #[test]
    fn export_strips_fixture_keeps_bone() {
        let mut paint = TissuePaint::new(4, 4);
        paint.set(1, 1, TissueKind::Bone);
        paint.set(2, 1, TissueKind::Fixture);
        paint.set(1, 2, TissueKind::ForceSensor);
        let body = StudioBody {
            paint,
            activated: true,
            graph: None,
            net: None,
        };
        let exp = export_body(&body).unwrap();
        assert_eq!(exp.paint.get(1, 1), TissueKind::Bone);
        assert_eq!(exp.paint.get(2, 1), TissueKind::Empty);
        assert_eq!(exp.paint.get(1, 2), TissueKind::Empty);
    }

    #[test]
    fn postcard_round_trip_with_net() {
        let mut paint = TissuePaint::new(8, 8);
        paint.set(2, 2, TissueKind::Bone);
        paint.set(3, 2, TissueKind::Muscle);
        let body = StudioBody::from_paint(paint);
        let net = StudioNet::for_muscles(1, 9);
        let exp = export_body_with_net(&body, Some(net.clone())).unwrap();
        let bytes = encode_body(&exp).unwrap();
        let back = decode_body(&bytes).unwrap();
        assert_eq!(back.schema, BODY_SCHEMA_VERSION);
        assert_eq!(back.paint.get(2, 2), TissueKind::Bone);
        assert_eq!(back.net.as_ref().unwrap().n_out, net.n_out);
    }
}
