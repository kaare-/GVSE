#ifndef TANK_H
#define TANK_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Hand-maintained to match crates/tank-ffi/src/lib.rs.
   cbindgen.toml regenerates this file when the ABI changes. */

typedef struct TankHandle TankHandle;
typedef struct Carried Carried;

typedef struct ToolEvent {
  uint8_t kind;      /* 1 paint 2 erase 3 pour 4 drain 5 fill 6 undo 7 swipe 8 seed
                      * 9 remove anything 10 glucose 11 calcium 12 detritus 13 clean
                      * 14 top-up 15 cut/root-trim 16 enclosure (material: 0 delete
                      * walls, 1 drain, 2 fill, 3 release → tank_take_enclosure_bucket) */
  uint8_t material;  /* wk_material id; TANK_GLASS (18) paints a glass wall;
                        for kind 16: enclosure op 0..3 */
  uint16_t x;
  uint16_t y;
  uint16_t r;
  uint8_t variant;   /* colour 0..3 */
  uint8_t _pad;
} ToolEvent;

typedef struct TankInput {
  float grav_x;
  float grav_y;
  uint8_t paused;
  uint8_t always_day;
  uint8_t swirl;     /* 0 = ignore tilt (default) */
  uint8_t _pad1;
  float light_slider;
  float t_base;
  int32_t flash_x;   /* physics cell; flash_r==0 means off */
  int32_t flash_y;
  int32_t flash_r;
  const ToolEvent *tools;
  uint32_t tool_count;
} TankInput;

typedef struct InspectRec {
  uint8_t hit;
  uint8_t material;
  uint8_t sat;
  uint8_t moisture;
  uint8_t variant;
  uint8_t _pad0;
  uint8_t _pad1;
  uint8_t _pad2;
  uint32_t owner;
  uint32_t vx;
  uint32_t vy;
} InspectRec;

/* kind: raw stream 1 seed 2 division 3 death 4 mutant 5 two-cell body
 * 10 lifeless 11 glass needs cleaning 12 brood released 13 shed 14 fragment
 * 15 regrown 16 predation 17 poison 18 signalling 19 heterotroph 20 fungus
 * 21 something moved 22 new species 23 warm-blooded 24 cloudy 25 life on land
 * 26 cutting took root 27 colour crown 28 eye 29 locomotion 30 escape
 * 31 chamber; milestones (once per tank) 32 first
 * life 33 first division 34 first death 35 new colour (per lineage)
 * 36 multicellular 37 giant cell 38 fungus 39 heterotroph 40 predation
 * 41 poison 42 signalling 43 something moved 44 new species
 * 45 warm-blooded 46 life on land 47 cutting took root 48 colour crown
 * 49 eye 50 locomotion 51 escape 52 chamber.
 * kind 0 = none. */
typedef struct EventRec {
  uint32_t kind;
  uint32_t arg0;
} EventRec;

typedef struct TelRec {
  uint32_t kind;
  uint32_t arg0;
} TelRec;

#define TANK_GLASS 18
#define TANK_NO_CELL 0xFFFFFFFFu
#define TANK_NO_SPECIES 0xFFFFu

/* Phase 3 inspect: the living cell under a pixel and its organism.
   cell == TANK_NO_CELL when there is none. bbox is the tracking rectangle
   in pixels (inclusive, top-left origin like tank_pixels). */
typedef struct CellInspectRec {
  uint32_t cell;
  uint32_t org;
  uint32_t genotype;
  uint16_t species;
  uint8_t mode;
  uint8_t energy_frac;     /* 0..255 of the store cap */
  uint32_t generation;
  uint32_t org_cells;
  uint32_t glued_kin;
  uint32_t age_ticks;
  uint32_t max_age_ticks;
  uint32_t children;
  uint8_t stress;
  uint8_t _pad0, _pad1, _pad2;
  uint16_t bbox_x0, bbox_y0, bbox_x1, bbox_y1;
} CellInspectRec;

/* Lineage card numbers; names via tank_species_name. */
typedef struct LineageRec {
  uint32_t cell;
  uint32_t org;
  uint32_t genotype;
  uint16_t species;
  uint16_t parent_species;  /* TANK_NO_SPECIES for a root */
  uint32_t generation;
  uint16_t mutations_since_seed;
  uint16_t species_depth;   /* the roman numeral */
  uint32_t kin_alive;
  uint32_t descendants_alive;
  uint32_t archived_relatives;
  uint32_t children;
} LineageRec;

/* One species of the tree-of-life view (id order; parents before children). */
typedef struct SpeciesNodeRec {
  uint16_t id;
  uint16_t parent;          /* TANK_NO_SPECIES for a root */
  uint16_t depth;
  uint8_t archived;
  uint8_t extinct;
  uint8_t r, g, b, _pad;    /* species swatch */
  uint32_t live_cells;
  uint32_t live_genotypes;
  uint32_t forms;           /* genotypes ever (live + folded): "N forms" */
  uint32_t born;            /* organisms ever born: edge thickness */
  uint64_t first_tick;
} SpeciesNodeRec;

/* Numbers of an archived specimen record; strings via tank_specimen_text. */
typedef struct SpecimenInfoRec {
  uint64_t id;
  uint64_t tank_seed;
  uint64_t tick;
  uint64_t archive_parent;
  uint8_t has_archive_parent;
  uint8_t r, g, b;
  uint16_t species_depth;
  uint16_t mutations_since_seed;
  uint32_t generation;
  uint32_t cells;
  uint32_t slots;
  uint32_t age_ticks;
  uint32_t max_age_ticks;
  uint32_t children;
  uint32_t kin_alive;
  uint32_t descendants_alive;
  uint16_t portrait_w;
  uint16_t portrait_h;
} SpecimenInfoRec;

TankHandle *tank_new(uint64_t seed, uint32_t w_cells, uint32_t h_cells);
TankHandle *tank_preset(uint64_t seed);
TankHandle *tank_empty(uint64_t seed);
uint32_t tank_seed_algae(TankHandle *handle, uint32_t px, uint32_t py);
uint32_t tank_live_cells(const TankHandle *handle);
uint32_t tank_swipe_film(TankHandle *handle, int32_t x0, int32_t y0, int32_t x1, int32_t y1, int32_t radius);
void tank_free(TankHandle *handle);
void tank_tick(TankHandle *handle, const TankInput *input);
uint8_t tank_undo(TankHandle *handle);
uint32_t tank_undo_depth(const TankHandle *handle);
const uint8_t *tank_pixels(TankHandle *handle, uint32_t *w, uint32_t *h);
uint64_t tank_pixel_hash(TankHandle *handle);
InspectRec tank_inspect(TankHandle *handle, uint32_t px, uint32_t py);
EventRec tank_poll_event(TankHandle *handle);
/* Scoop (Phase 3). Pixel coordinates, radius in pixels. Null = nothing
   inside. tank_release returns 1 and consumes the bucket; 0 (no room or
   out of bounds) leaves it valid for another try. tank_discard throws it
   away (consumes). tank_carried_free only frees memory — prefer discard. */
Carried *tank_scoop(TankHandle *handle, int32_t cx, int32_t cy, int32_t r);
uint32_t tank_carried_cells(const Carried *carried);
uint8_t tank_release(TankHandle *handle, Carried *carried, int32_t x, int32_t y);
void tank_discard(TankHandle *handle, Carried *carried);
void tank_carried_free(Carried *carried);
/* Remove anything under a brush (physics cell coords, bottom-left origin). */
uint32_t tank_remove_at(TankHandle *handle, int32_t vx, int32_t vy, int32_t r);

/* Inspect and lineage (Phase 3). Strings are UTF-8, NUL-terminated when
   they fit; the return value is the byte length without the NUL, so a
   call with cap 0 measures. */
CellInspectRec tank_inspect_cell(const TankHandle *handle, uint32_t px, uint32_t py);
uint8_t tank_org_bbox(const TankHandle *handle, uint32_t org, uint16_t *x0, uint16_t *y0, uint16_t *x1, uint16_t *y1);
LineageRec tank_lineage(const TankHandle *handle, uint32_t cell);
uint32_t tank_species_name(const TankHandle *handle, uint16_t species, uint8_t *buf, uint32_t cap);
uint32_t tank_species_count(const TankHandle *handle);
/* Saved species and the ancestors that connect them. Unsaved life is omitted. */
uint32_t tank_species_tree(const TankHandle *handle, SpeciesNodeRec *out, uint32_t cap);
uint8_t tank_mark_species_archived(TankHandle *handle, uint16_t species, uint8_t on);

/* Archive (Phase 3). A specimen is an opaque byte record the shell stores
   (documents dir) and shares. tank_specimen archives the organism of a
   living cell (two-call sizing like tank_save) and pins its species.
   which for tank_specimen_text: 0 display name, 1 generated species name,
   2 lineage "A → B → C", 3 notes, 4 the whole page as text. The portrait
   is RGBA8 portrait_w × portrait_h at 1× (draw it 8× nearest-neighbour). */
size_t tank_specimen(TankHandle *handle, uint32_t cell, uint8_t *buf, size_t cap);
SpecimenInfoRec tank_specimen_info(const uint8_t *bytes, size_t len);
uint32_t tank_specimen_text(const uint8_t *bytes, size_t len, uint8_t which, uint8_t *buf, uint32_t cap);
size_t tank_specimen_portrait(const uint8_t *bytes, size_t len, uint8_t *out, size_t cap);
size_t tank_specimen_edit(const uint8_t *bytes, size_t len, const uint8_t *name, size_t name_len,
                          const uint8_t *notes, size_t notes_len, uint8_t *out, size_t cap);
uint32_t tank_seed_specimen(TankHandle *handle, const uint8_t *bytes, size_t len, uint32_t px, uint32_t py);
uint32_t tank_seed_specimen_copies(TankHandle *handle, const uint8_t *bytes, size_t len, uint32_t n);
size_t tank_save(TankHandle *handle, uint8_t *buf, size_t cap);
TankHandle *tank_load(const uint8_t *bytes, size_t len);
uint64_t tank_tick_count(const TankHandle *handle);
float tank_day_factor(const TankHandle *handle);
uint8_t tank_is_day(const TankHandle *handle);
uint64_t tank_seed(const TankHandle *handle);
float tank_t_base(const TankHandle *handle);
float tank_temperature(const TankHandle *handle);
uint8_t tank_humidity_at(const TankHandle *handle, int32_t vx, int32_t vy);
float tank_t_local_at(const TankHandle *handle, int32_t vx, int32_t vy);
float tank_cloud_mean(const TankHandle *handle);
float tank_light_slider(const TankHandle *handle);
/* kind: 0 moss, 1 plant, 2 crown, 3 rooting, 4 heater. Two-call sizing. */
size_t tank_starter_specimen(uint8_t kind, uint8_t *buf, size_t cap);
/* After tool 16 / op 3 (release); null when nothing pending. */
Carried *tank_take_enclosure_bucket(TankHandle *handle);
void tank_set_consent(TankHandle *handle, uint8_t on);
void tank_telemetry_kill(TankHandle *handle, uint8_t killed);
uint8_t tank_telemetry_push(TankHandle *handle, uint32_t kind, uint32_t arg0);
uint32_t tank_telemetry_drain(TankHandle *handle, TelRec *out, uint32_t cap);

/* 7 rows of 5 bits. Row 0 is in the lowest bits. Bit 4 of a row is the
   leftmost pixel. 0 is the blank glyph (space). */
uint64_t tank_glyph(uint32_t codepoint);

#ifdef __cplusplus
}
#endif

#endif /* TANK_H */
