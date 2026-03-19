const SOUND_FILES = {
  send: "/aethos_send.wav",
  receive: "/aethos_receive.wav",
  encounter: "/aethos_encounter.wav",
  sync: "/aethos_sync.wav",
  error: "/aethos_error.wav"
};

const ENCOUNTER_COOLDOWN_MS = 2000;
const GLOBAL_COOLDOWN_MS = 120;

class SoundManager {
  constructor() {
    this.audioByKind = new Map();
    this.lastPlayedAt = new Map();
    this.lastAnyPlayedAt = 0;
  }

  play(kind) {
    const src = SOUND_FILES[kind];
    if (!src) return;

    const now = Date.now();
    const lastAny = this.lastAnyPlayedAt;
    if (now - lastAny < GLOBAL_COOLDOWN_MS) {
      console.debug(`sound_suppressed: ${kind}_global_cooldown`);
      return;
    }

    if (kind === "encounter") {
      const lastEncounter = this.lastPlayedAt.get("encounter") || 0;
      if (now - lastEncounter < ENCOUNTER_COOLDOWN_MS) {
        console.debug("sound_suppressed: encounter_cooldown");
        return;
      }
    }

    const audio = this.getOrCreateAudio(kind, src);
    if (!audio) return;

    this.lastAnyPlayedAt = now;
    this.lastPlayedAt.set(kind, now);

    try {
      audio.currentTime = 0;
    } catch {
      // ignore non-fatal seek issues
    }

    audio.play().then(() => {
      console.debug(`sound_played: ${kind}`);
    }).catch(() => {
      console.debug(`sound_play_failed: ${kind}`);
    });
  }

  getOrCreateAudio(kind, src) {
    if (this.audioByKind.has(kind)) {
      return this.audioByKind.get(kind);
    }

    try {
      const audio = new Audio(src);
      audio.preload = "auto";
      this.audioByKind.set(kind, audio);
      return audio;
    } catch {
      console.debug(`sound_missing_or_unavailable: ${kind}`);
      return null;
    }
  }
}

export const soundManager = new SoundManager();
