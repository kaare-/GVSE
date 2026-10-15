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
  uint8_t kind;      /* 1 paint 2 erase 3 pour 4 drain 5 fill 6 undo 7 swipe 8 seed 9 remove 10 glucose 11 calcium 12 detritus 13 clean */
  uint8_t material;
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

typedef struct EventRec {
  uint32_t kind;
  uint32_t arg0;
} EventRec;

typedef struct TelRec {
  uint32_t kind;
  uint32_t arg0;
} TelRec;

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
Carried *tank_scoop(TankHandle *handle, int32_t cx, int32_t cy, int32_t r);
uint8_t tank_release(TankHandle *handle, Carried *carried, int32_t x, int32_t y);
void tank_carried_free(Carried *carried);
size_t tank_save(TankHandle *handle, uint8_t *buf, size_t cap);
TankHandle *tank_load(const uint8_t *bytes, size_t len);
uint64_t tank_tick_count(const TankHandle *handle);
float tank_day_factor(const TankHandle *handle);
uint8_t tank_is_day(const TankHandle *handle);
uint64_t tank_seed(const TankHandle *handle);
float tank_t_base(const TankHandle *handle);
float tank_light_slider(const TankHandle *handle);
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
