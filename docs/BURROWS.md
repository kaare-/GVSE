# Burrows: dig API

*Stage 9. Builds on void annotations from stage 7 (`UNDERGROUND.md`).*

## API

```rust
world.dig(column_x, target_y, volume_kg) -> DigResult
```

- Removes up to `volume_kg` from the diggable solid layer at `target_y`
- Grows or spawns a `Void { origin: Burrow }` at that elevation
- Deposits the same mass on the column surface as a tailings mound
- If the contiguous void span exceeds the roof material's
  `roof_span_max_m`, opens the void to the surface (trench / doline)

Sand and clay have `roof_span_max_m = 0`, so shallow digs in topsoil
always trench — moles make open runs, not stone tunnels.

## Mass

Layer mass is conserved (removed then redeposited as tailings). Void
height raises `surface_y` by the mound. No new audit buckets.

## Scenario

**E13** — dig produces surface tailings and a burrow/trench void with
conserved column mass.
