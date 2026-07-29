# Palette

*Frozen module colour atlas and pixel grammar. `palette-lock` in the
Organism Kernel plan.*

## Freeze contract

- **RGB hex is a save-format identifier.** Once a module ships in
  Phase 2, its hex is frozen forever. Blueprints in `.gvsecrt` files
  and postcard saves reference the module by its `ModuleId` enum
  variant, and the palette maps that variant to the frozen hex.
- **Renaming is fine, renumbering is not.** `ModuleId` values must be
  stable in `#[repr(u8)]` for save-compat; new modules append.
- **Add colours slowly.** A new colour costs the reader another mental
  slot. Reserve a hex now only if you can name its job in one line.

## Pixel grammar

| Mark | Size | Meaning |
|------|------|---------|
| Module | 1×1 px | One organelle. Every solid module of the creature. |
| Neural soma | 2×2 px | Rudimentary "brain". Physically wider than any single module. |
| Axon | 1 px line, gray | Signal path between modules and soma. |
| Hypha | 1 px line, cream | Fungal thread through soil / litter / standing dead. Same line grammar as an axon, different colour, carries nutrients not control. |
| Attach / holdfast | 1 px overlay | Pink pixel may share a Mid-lane cell with an olive stem (see [`LANES.md`](LANES.md)). |
| Body hint | faint halo | Optional readability aid when biomass demands it. Not a module, not saved. |

No anti-aliased circles. No gradients on the creature. Soft fields
belong to the *world* (temperature, humidity, chem, light), not the
body.

## Module IDs and hex

`ModuleId` is a `#[repr(u8)]` enum. Values below the "reserved" line
are frozen. Values above are hex slots we like the look of but have
not committed a job to.

### Core (needed by Sets A–E)

| ID | Name | Hex | Job |
|----|------|-----|-----|
| `0x00` | Nucleus | `#000000` | Always on. Genome home. Emits life stats (energy, health, clock phase). Baseline upkeep. |
| `0x01` | Photosystem | `#2ECC40` | Light + water/air → energy. Emits `production` signal. |
| `0x02` | Chemosystem | `#B58900` | Consume one `ChemType` → energy. Alternate / add-on to photosystem. |
| `0x03` | ChemoSensor | `#0A6C74` | Reads a gene-chosen `ChemType` local level or gradient → neural input. |
| `0x04` | ChemoEmitter | `#39CCCC` | Releases a gene-chosen `ChemType` into the water at a controlled rate. |
| `0x05` | NeuralSoma | `#7F7F7F` | 2×2 gray square. Reads wires in, writes wires out. |
| `0x06` | Axon | `#AAAAAA` | 1-px gray line between modules and soma. |
| `0x07` | Buoyancy | `#7FDBFF` | Regulates float/sink (target depth / density). Water-only organs. |
| `0x08` | TempTolerance | `#FF851B` | Widens comfort temperature band or shifts optimum. |
| `0x09` | Store | `#EFEFEF` | Pale energy buffer for night / lean times. |
| `0x0A` | Digest | `#8B2E2E` | Convert local `organic` litter (or contact food later) → energy. |
| `0x0B` | Hypha | `#F1E6C4` | Cream 1-px thread through soil, litter, or standing dead. Extends digest reach. |
| `0x0C` | Motility | `#B10DC9` | Horizontal / directed move. Reserved for a later stage; keep the slot. |
| `0x0D` | Root | `#7A4B2A` | Sienna. Anchors, drinks moisture, elongates down a moisture gradient. |
| `0x0E` | Stem | `#556B2F` | Olive. Stacks upward, holds leaves into the light column, casts shade. Substrate for epiphytes. |
| `0x0F` | Holdfast | `#FF3D9A` | Pink. Grips another organism's olive stem (rock later). No ground root required. |
| `0x13` | Skin | `#FFDBAC` | Animal outer layer. Studio-live (Wave K); every pixel carries `PixelTraits`. World material + decay in Wave L. |
| `0x14` | Muscle | `#C33C3C` | Animal motility tissue. Studio-live (Wave K); world material + decay in Wave L. |
| `0x15` | Bone | `#EFE7DA` | Skeletal element. Studio-live (Wave K); world material + decay in Wave L. |

Every painted module cell also stores a [`PixelTraits`](GENES.md)
payload (mass, density, stiffness, …). Aggregates form the body plan;
see [`GENES.md`](GENES.md).

### Reserved (frozen slots, no code yet)

| ID | Name | Hex | Note |
|----|------|-----|------|
| `0x10` | ReproSpore | `#D0B0FF` | Dispersal packet (seed / spore). Draft; may fold into `Nucleus` behaviour. |
| `0x11` | Fruit | `#E85D75` | Reproductive body, animal-attractant vector for Phase 7+. |
| `0x12` | Bark | `#3E2E1F` | Woody protective sheath; requires stem integrity work first. |

Reserved slots exist so a `.gvsecrt` from a future build never
collides with a live module ID. They may be renamed or repurposed
until they ship, then they freeze the same way as core.

## Colour groupings by job

For editor palette drawer layout (see [`EDITOR.md`](EDITOR.md)):

- **Identity & metabolism** — Nucleus, Photosystem, Chemosystem, Store.
- **Chemistry** — ChemoSensor, ChemoEmitter.
- **Nervous system** — NeuralSoma, Axon.
- **Physiology / niche** — Buoyancy, TempTolerance, Motility.
- **Detritus loop** — Digest, Hypha.
- **Land body** — Root, Stem, Holdfast.
- **Animal tissue** — Skin, Muscle, Bone.

Colour choices were picked to be readable at 1× zoom on a mid-gray
world background, distinguish adjacent categories (green / olive not
identical, teal-sense / teal-emit distinguishable, sienna / brown-red
distinct), and stay MS-Paint plausible (no anti-alias, no gradients).

## Occupancy rules

- One solid module per cell **within a depth lane** (see
  [`LANES.md`](LANES.md)).
- `Holdfast` (pink) may layer on an existing `Stem` (olive) in the Mid
  lane — this is the attach exception.
- `Axon` and `Hypha` are 1-px lines, not solid cells; they may cross
  or share pixels with adjacent modules of the same organism.
- `NeuralSoma` occupies **four cells** in a 2×2 block.

## Draw order (screen ↔ blueprint)

Editor canvas draws bottom → top for stacking:

1. Substrate hint (world background, not a module).
2. Solid modules by lane (Back → Mid → Fore).
3. Axons and hyphae (1-px lines over solids).
4. Attach overlays (pink).
5. Body hint halo (optional, never saved).

Runtime rendering uses the same order so a Fore animal walking past a
Mid trunk reads correctly (see [`LANES.md`](LANES.md)).

## Save-format guarantee

A `Blueprint` serialises as:

```
Blueprint {
    modules: Vec<(x: i16, y: i16, module: ModuleId)>,
    wires: Vec<(from_index: u16, to_index: u16, kind: WireKind)>,
    genome: Genome,
    schema_version: u16,
}
```

`ModuleId` is `#[repr(u8)]`, so a `.gvsecrt` from any future build is
readable as long as it does not reference a slot the current binary
has not compiled in yet. Old blueprints on a new build always load;
new blueprints on an old build may reject a reserved-slot module with
a clear "unknown module 0xNN" error.
