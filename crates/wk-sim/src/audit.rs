use wk_world::world::MassAudit;

pub fn assert_mass_non_negative(audit: &MassAudit) -> bool {
    audit.by_material.iter().all(|&m| m >= 0)
}

pub fn compute_bookkeeping_delta(audit_before: &MassAudit, audit_after: &MassAudit) -> i64 {
    let mass_delta = audit_after.total_tracked() - audit_before.total_tracked();
    let inject_delta = (audit_after.rain_inject_total - audit_before.rain_inject_total)
        + (audit_after.sea_inject_total - audit_before.sea_inject_total);
    let out_delta = (audit_after.evap_out_total - audit_before.evap_out_total)
        + (audit_after.boundary_out_total - audit_before.boundary_out_total);
    mass_delta - inject_delta + out_delta
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_system_invariant() {
        let a = MassAudit::default();
        let b = MassAudit::default();
        assert_eq!(compute_bookkeeping_delta(&a, &b), 0);
    }
}
