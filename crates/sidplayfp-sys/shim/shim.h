/* SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * A C interface to the parts of libsidplayfp the player uses. No C++ exception
 * leaves these functions: each catches everything and reports failure. */

#ifndef HELIKODU_SIDPLAYFP_SHIM_H
#define HELIKODU_SIDPLAYFP_SHIM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct hk_sid hk_sid;
typedef struct hk_sid_db hk_sid_db;

/* Loads a PSID or RSID file from memory and prepares an emulator rendering
 * 16-bit stereo at `rate` Hz. NULL on failure, with the reason in `error`. */
hk_sid *hk_sid_open(const uint8_t *data, uint32_t length, uint32_t rate, char *error,
                    size_t error_len);
void hk_sid_free(hk_sid *sid);

/* The number of songs and the default song (counted from 1). */
unsigned hk_sid_songs(const hk_sid *sid);
unsigned hk_sid_start_song(const hk_sid *sid);
/* The file's text: 0 title, 1 author, 2 released; NULL when absent. Valid until the
 * tune is freed. */
const char *hk_sid_info_string(const hk_sid *sid, unsigned index);
/* The file format, such as "PlaySID one-file format (PSID)". */
const char *hk_sid_format(const hk_sid *sid);
/* The tune's MD5 in the form the HVSC song-length database uses (32 hex digits and
 * a NUL); 0 on failure. */
int hk_sid_md5(hk_sid *sid, char *out);

/* Installs ROM images (KERNAL 8192, BASIC 8192, character 4096 bytes); any may be
 * NULL. They are copied. Takes effect at the next hk_sid_select. */
void hk_sid_set_roms(hk_sid *sid, const uint8_t *kernal, const uint8_t *basic,
                     const uint8_t *chargen);

/* Starts song `song` (counted from 1) from its beginning. 0 on failure, with the
 * reason in `error`. */
int hk_sid_select(hk_sid *sid, unsigned song, char *error, size_t error_len);

/* Renders up to `frames` stereo frames into `out` (interleaved). Returns the number
 * rendered, or -1 on failure (see hk_sid_error). */
long hk_sid_render(hk_sid *sid, int16_t *out, size_t frames);
/* The last error of the emulator. */
const char *hk_sid_error(const hk_sid *sid);

/* Opens an HVSC song-length database (Songlengths.md5). NULL on failure. */
hk_sid_db *hk_sid_db_open(const char *path, char *error, size_t error_len);
void hk_sid_db_free(hk_sid_db *db);
/* The length of `song` (counted from 1) of the tune in milliseconds, or -1 when the
 * database does not know it. */
long hk_sid_db_length_ms(hk_sid_db *db, hk_sid *sid, unsigned song);

#ifdef __cplusplus
}
#endif

#endif
