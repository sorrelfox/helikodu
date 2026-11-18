/* SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * See shim.h. */

#include "shim.h"

#include <cstring>
#include <new>
#include <string>
#include <vector>

#include <sidplayfp/SidConfig.h>
#include <sidplayfp/SidDatabase.h>
#include <sidplayfp/SidInfo.h>
#include <sidplayfp/SidTune.h>
#include <sidplayfp/SidTuneInfo.h>
#include <sidplayfp/builders/residfp.h>
#include <sidplayfp/sidplayfp.h>
#include <sidplayfp/sidversion.h>

/* play(cycles) and buffers() (2.14) give each chip's samples; the shim mixes them
 * itself, without the random dither of the library's mixer. */
#if LIBSIDPLAYFP_VERSION_MAJ > 2 || (LIBSIDPLAYFP_VERSION_MAJ == 2 && LIBSIDPLAYFP_VERSION_MIN >= 14)
#define HK_SID_BUFFERS 1
#endif

namespace {

/* Cycles emulated per step of the mixer API: about 5 ms. */
const unsigned STEP_CYCLES = 5000;

void copy_message(const char *message, char *out, size_t len) {
    if (out == nullptr || len == 0) {
        return;
    }
    if (message == nullptr) {
        message = "unknown error";
    }
    std::strncpy(out, message, len - 1);
    out[len - 1] = '\0';
}

}  // namespace

struct hk_sid {
    SidTune tune;
    sidplayfp player;
    ReSIDfpBuilder builder;
    uint32_t rate;
    std::vector<uint8_t> kernal, basic, chargen;
    std::vector<int16_t> pending;
    size_t pending_at;
    std::string error;

    hk_sid(const uint8_t *data, uint32_t length, uint32_t rate)
        : tune(data, length), builder("helikodu"), rate(rate), pending_at(0) {}
};

struct hk_sid_db {
    SidDatabase db;
};

extern "C" {

hk_sid *hk_sid_open(const uint8_t *data, uint32_t length, uint32_t rate, char *error,
                    size_t error_len) {
    try {
        hk_sid *sid = new hk_sid(data, length, rate);
        if (!sid->tune.getStatus()) {
            copy_message(sid->tune.statusString(), error, error_len);
            delete sid;
            return nullptr;
        }
        sid->builder.create(sid->player.info().maxsids());
        if (!sid->builder.getStatus()) {
            copy_message(sid->builder.error(), error, error_len);
            delete sid;
            return nullptr;
        }
        SidConfig config;
        config.frequency = rate;
        config.playback = SidConfig::STEREO;
        config.samplingMethod = SidConfig::RESAMPLE_INTERPOLATE;
        config.fastSampling = false;
        /* A fixed power-on delay, so that a song starts the same way every time and a
         * seek (a restart and a run to the frame) lands where a straight run is. */
        config.powerOnDelay = 0;
        config.sidEmulation = &sid->builder;
        if (!sid->player.config(config)) {
            copy_message(sid->player.error(), error, error_len);
            delete sid;
            return nullptr;
        }
        return sid;
    } catch (const std::exception &e) {
        copy_message(e.what(), error, error_len);
    } catch (...) {
        copy_message("libsidplayfp failed", error, error_len);
    }
    return nullptr;
}

void hk_sid_free(hk_sid *sid) {
    try {
        delete sid;
    } catch (...) {
    }
}

unsigned hk_sid_songs(const hk_sid *sid) {
    try {
        const SidTuneInfo *info = sid->tune.getInfo();
        return info != nullptr ? info->songs() : 0;
    } catch (...) {
        return 0;
    }
}

unsigned hk_sid_start_song(const hk_sid *sid) {
    try {
        const SidTuneInfo *info = sid->tune.getInfo();
        return info != nullptr ? info->startSong() : 0;
    } catch (...) {
        return 0;
    }
}

const char *hk_sid_info_string(const hk_sid *sid, unsigned index) {
    try {
        const SidTuneInfo *info = sid->tune.getInfo();
        if (info == nullptr || index >= info->numberOfInfoStrings()) {
            return nullptr;
        }
        return info->infoString(index);
    } catch (...) {
        return nullptr;
    }
}

const char *hk_sid_format(const hk_sid *sid) {
    try {
        const SidTuneInfo *info = sid->tune.getInfo();
        return info != nullptr ? info->formatString() : nullptr;
    } catch (...) {
        return nullptr;
    }
}

int hk_sid_md5(hk_sid *sid, char *out) {
    try {
        return sid->tune.createMD5New(out) != nullptr ? 1 : 0;
    } catch (...) {
        return 0;
    }
}

void hk_sid_set_roms(hk_sid *sid, const uint8_t *kernal, const uint8_t *basic,
                     const uint8_t *chargen) {
    try {
        sid->kernal.assign(kernal, kernal != nullptr ? kernal + 8192 : kernal);
        sid->basic.assign(basic, basic != nullptr ? basic + 8192 : basic);
        sid->chargen.assign(chargen, chargen != nullptr ? chargen + 4096 : chargen);
        sid->player.setRoms(sid->kernal.empty() ? nullptr : sid->kernal.data(),
                            sid->basic.empty() ? nullptr : sid->basic.data(),
                            sid->chargen.empty() ? nullptr : sid->chargen.data());
    } catch (...) {
    }
}

int hk_sid_select(hk_sid *sid, unsigned song, char *error, size_t error_len) {
    try {
        sid->tune.selectSong(song);
        if (!sid->player.load(&sid->tune)) {
            copy_message(sid->player.error(), error, error_len);
            return 0;
        }
        sid->pending.clear();
        sid->pending_at = 0;
        return 1;
    } catch (const std::exception &e) {
        copy_message(e.what(), error, error_len);
    } catch (...) {
        copy_message("libsidplayfp failed", error, error_len);
    }
    return 0;
}

long hk_sid_render(hk_sid *sid, int16_t *out, size_t frames) {
    try {
        size_t done = 0;
        while (done < frames) {
            if (sid->pending_at < sid->pending.size()) {
                size_t available = (sid->pending.size() - sid->pending_at) / 2;
                size_t take = available < frames - done ? available : frames - done;
                std::memcpy(out + done * 2, sid->pending.data() + sid->pending_at,
                            take * 2 * sizeof(int16_t));
                sid->pending_at += take * 2;
                done += take;
                continue;
            }
#ifdef HK_SID_BUFFERS
            int samples = sid->player.play(STEP_CYCLES);
            if (samples < 0) {
                sid->error = sid->player.error();
                return -1;
            }
            unsigned chips = sid->player.installedSIDs();
            if (chips < 1 || chips > 3) {
                sid->error = "unexpected number of SID chips";
                return -1;
            }
            short *chip[3] = {nullptr, nullptr, nullptr};
            sid->player.buffers(chip);
            sid->pending.resize(static_cast<size_t>(samples) * 2);
            for (int i = 0; i < samples; i++) {
                int left, right;
                switch (chips) {
                case 1:
                    left = right = chip[0][i];
                    break;
                case 2:
                    left = chip[0][i];
                    right = chip[1][i];
                    break;
                default:
                    /* The middle chip in the centre, as libsidplayfp mixes three. */
                    left = (2 * chip[0][i] + chip[1][i]) / 3;
                    right = (2 * chip[2][i] + chip[1][i]) / 3;
                    break;
                }
                sid->pending[2 * i] = static_cast<int16_t>(left);
                sid->pending[2 * i + 1] = static_cast<int16_t>(right);
            }
#else
            sid->pending.resize(512);
            uint_least32_t got = sid->player.play(sid->pending.data(), 512);
            if (got < 512 || !sid->player.isPlaying()) {
                sid->error = sid->player.error();
                return -1;
            }
            sid->pending.resize(got);
#endif
            sid->pending_at = 0;
        }
        return static_cast<long>(done);
    } catch (const std::exception &e) {
        sid->error = e.what();
    } catch (...) {
        sid->error = "libsidplayfp failed";
    }
    return -1;
}

const char *hk_sid_error(const hk_sid *sid) {
    return sid->error.c_str();
}

hk_sid_db *hk_sid_db_open(const char *path, char *error, size_t error_len) {
    try {
        hk_sid_db *db = new hk_sid_db();
        if (!db->db.open(path)) {
            copy_message(db->db.error(), error, error_len);
            delete db;
            return nullptr;
        }
        return db;
    } catch (const std::exception &e) {
        copy_message(e.what(), error, error_len);
    } catch (...) {
        copy_message("cannot read the song-length database", error, error_len);
    }
    return nullptr;
}

void hk_sid_db_free(hk_sid_db *db) {
    try {
        delete db;
    } catch (...) {
    }
}

long hk_sid_db_length_ms(hk_sid_db *db, hk_sid *sid, unsigned song) {
    try {
        char md5[SidTune::MD5_LENGTH + 1];
        if (sid->tune.createMD5New(md5) == nullptr) {
            return -1;
        }
        long length = db->db.lengthMs(md5, song);
        if (length <= 0) {
            /* Older databases list the tunes by the old MD5. */
            if (sid->tune.createMD5(md5) == nullptr) {
                return -1;
            }
            length = db->db.lengthMs(md5, song);
        }
        return length > 0 ? length : -1;
    } catch (...) {
        return -1;
    }
}

}  // extern "C"
