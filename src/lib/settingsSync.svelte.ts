// Settings sync's status, kept current from the core's `settings-sync-status` events
// (src-tauri/src/sync). Settings → Sync shows it; the palette's "Sync settings now" uses it.
import { onSettingsSyncStatus, settingsSyncApi, type SettingsSyncStatus } from "./api";

class SettingsSync {
  status = $state<SettingsSyncStatus | null>(null);

  /** Sync is on for this device. */
  get on() {
    return !!this.status && this.status.state !== "off";
  }

  /** Loads the status and follows it. Returns the unlisten function. */
  init() {
    settingsSyncApi.status().then((s) => (this.status = s)).catch(() => {});
    return onSettingsSyncStatus((s) => (this.status = s));
  }

  async syncNow() {
    this.status = await settingsSyncApi.syncNow();
  }
}

export const settingsSync = new SettingsSync();
