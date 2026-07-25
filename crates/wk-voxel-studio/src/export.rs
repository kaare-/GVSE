//! Strip studio-only pixels into a world-ready body payload.

use serde::{Deserialize, Serialize};

use crate::tissue::{StudioBody, TissueKind, TissuePaint};

pub const BODY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedBody {
    pub schema: u32,
    pub paint: TissuePaint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    EmptyBody,
}

/// Drop fixture / force-sensor / empty; keep morphology + joints + nerve.
///
/// S0 exports paint before activate exists. S1+ may require
/// `body.activated` before calling this.
pub fn export_body(body: &StudioBody) -> Result<ExportedBody, ExportError> {
    let mut out = TissuePaint::new(body.paint.width, body.paint.height);
    let mut any = false;
    for y in 0..body.paint.height {
        for x in 0..body.paint.width {
            let k = body.paint.get(x, y);
            if k.exportable() {
                // Joints export as themselves; fixture/sensor already filtered.
                out.set(x, y, k);
                any = true;
            } else if matches!(k, TissueKind::Empty | TissueKind::Fixture | TissueKind::ForceSensor)
            {
                // stripped
            }
        }
    }
    if !any {
        return Err(ExportError::EmptyBody);
    }
    Ok(ExportedBody {
        schema: BODY_SCHEMA_VERSION,
        paint: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
        };
        let exp = export_body(&body).unwrap();
        assert_eq!(exp.paint.get(1, 1), TissueKind::Bone);
        assert_eq!(exp.paint.get(2, 1), TissueKind::Empty);
        assert_eq!(exp.paint.get(1, 2), TissueKind::Empty);
    }
}
