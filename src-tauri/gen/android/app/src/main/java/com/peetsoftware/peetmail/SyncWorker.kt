package com.peetsoftware.peetmail

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * Checks for mail with PeetMail closed (docs/ANDROID.md Stage 5). WorkManager runs it every
 * 15 minutes or hour (Settings → Check for mail) when there's a network, which Android allows
 * PeetMail for the length of the job. The work is all in Rust (src-tauri/src/background.rs):
 * this loads the library, hands it the key for saved sign-ins, and shows the notifications
 * it returns.
 */
class SyncWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
  override fun doWork(): Result {
    val dataDir = applicationContext.dataDir
    val key = try {
      SignInKey.load(dataDir)
    } catch (e: Exception) {
      // Rust reports the accounts it then can't sync; mail already here still notifies.
      Log.w(TAG, "couldn't unwrap the key for saved sign-ins", e)
      null
    }
    val out = try {
      System.loadLibrary("peetmail_lib")
      JSONObject(runOnce(dataDir.absolutePath, key))
    } catch (e: Throwable) {
      Log.e(TAG, "the background check failed", e)
      return Result.success() // the next period tries again; a retry would only add runs
    }
    out.optString("error").takeIf { it.isNotEmpty() }?.let { Log.w(TAG, "background check: $it") }
    out.optJSONArray("toasts")?.let { list ->
      for (i in 0 until list.length()) show(list.getJSONObject(i))
    }
    return Result.success()
  }

  private fun show(t: JSONObject) {
    val context = applicationContext
    if (!NotificationManagerCompat.from(context).areNotificationsEnabled()) return
    ensureChannel(context)
    val id = (System.currentTimeMillis() and 0x7fffffff).toInt() xor t.optString("title").hashCode()
    // A tap opens PeetMail on the conversation (PeetPlugin.onIntent).
    val intent = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
    if (!t.isNull("conversationId")) {
      intent.putExtra(EXTRA_ACCOUNT, t.getLong("accountId")).putExtra(EXTRA_CONVERSATION, t.getString("conversationId"))
    }
    val tap = PendingIntent.getActivity(context, id, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    val body = t.optString("body")
    val n = NotificationCompat.Builder(context, CHANNEL_ID)
      .setSmallIcon(android.R.drawable.ic_dialog_email)
      .setContentTitle(t.optString("title"))
      .setContentText(body.lineSequence().firstOrNull() ?: "")
      .setStyle(NotificationCompat.BigTextStyle().bigText(body))
      .setContentIntent(tap)
      .setAutoCancel(true)
      .build()
    try {
      NotificationManagerCompat.from(context).notify(id, n)
    } catch (e: SecurityException) {
      Log.w(TAG, "notifications aren't allowed", e)
    }
  }

  companion object {
    private const val TAG = "PeetMail"
    private const val WORK_NAME = "peetmail-check-mail"
    /** tauri-plugin-notification's channel, so all of PeetMail's notifications share one
     *  setting. Made here first if the app hasn't been opened since install. */
    private const val CHANNEL_ID = "default"
    const val EXTRA_ACCOUNT = "com.peetsoftware.peetmail.accountId"
    const val EXTRA_CONVERSATION = "com.peetsoftware.peetmail.conversationId"

    /** Runs one check: background.rs. Returns `{"toasts": [...]}` or `{"error": "..."}`. */
    @JvmStatic
    external fun runOnce(dataDir: String, key: ByteArray?): String

    /** Every `minutes` (Android's floor is 15), or never for 0. Keeps the schedule it has
     *  when nothing changed, so opening PeetMail doesn't push the next run back. */
    fun schedule(context: Context, minutes: Int) {
      val work = WorkManager.getInstance(context)
      if (minutes <= 0) {
        work.cancelUniqueWork(WORK_NAME)
        return
      }
      val request = PeriodicWorkRequestBuilder<SyncWorker>(minutes.toLong(), TimeUnit.MINUTES)
        .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
        .build()
      work.enqueueUniquePeriodicWork(WORK_NAME, ExistingPeriodicWorkPolicy.UPDATE, request)
    }

    private fun ensureChannel(context: Context) {
      val manager = context.getSystemService(NotificationManager::class.java)
      if (manager.getNotificationChannel(CHANNEL_ID) == null) {
        manager.createNotificationChannel(NotificationChannel(CHANNEL_ID, "Default", NotificationManager.IMPORTANCE_DEFAULT))
      }
    }
  }
}
