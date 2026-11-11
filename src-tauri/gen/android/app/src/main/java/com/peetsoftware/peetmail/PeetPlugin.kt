package com.peetsoftware.peetmail

import android.app.Activity
import android.content.ContentValues
import android.content.Intent
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.provider.Settings
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.webkit.MimeTypeMap
import android.webkit.WebView
import androidx.core.content.FileProvider
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import org.json.JSONObject
import java.io.File
import java.security.KeyStore
import java.util.UUID
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * What PeetMail's Rust core can't do on Android by itself (docs/ANDROID.md Stage 4), called
 * from src-tauri/src/android.rs:
 * - wrap/unwrap the key for saved sign-ins with a key that never leaves the Android Keystore;
 * - open, save and share files through content URIs;
 * - catch what other apps hand PeetMail: "Share → PeetMail", the sign-in redirect, and taps on
 *   its notifications. These wait in `pending` until the page asks for them (`takePending`);
 *   the page is told to ask by a `peetmail-pending` event.
 */
@TauriPlugin
class PeetPlugin(private val activity: Activity) : Plugin(activity) {
  private var webView: WebView? = null
  private val pending = JSObject()

  override fun load(webView: WebView) {
    this.webView = webView
    activity.intent?.let { onIntent(it) }
  }

  override fun onNewIntent(intent: Intent) {
    onIntent(intent)
  }

  // ----- the Keystore -----

  private fun keystoreKey(): SecretKey {
    val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    (store.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
    val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
    gen.init(
      KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setKeySize(256)
        .build()
    )
    return gen.generateKey()
  }

  /** `data` (base64) encrypted with the Keystore key: the 12-byte nonce, then the sealed bytes. */
  @Command
  fun wrap(invoke: Invoke) {
    try {
      val data = Base64.decode(invoke.getArgs().getString("data"), Base64.NO_WRAP)
      val cipher = Cipher.getInstance(TRANSFORMATION)
      cipher.init(Cipher.ENCRYPT_MODE, keystoreKey())
      val out = cipher.iv + cipher.doFinal(data)
      invoke.resolve(JSObject().put("data", Base64.encodeToString(out, Base64.NO_WRAP)))
    } catch (e: Exception) {
      invoke.reject("Android Keystore: ${e.message}")
    }
  }

  @Command
  fun unwrap(invoke: Invoke) {
    try {
      val blob = Base64.decode(invoke.getArgs().getString("data"), Base64.NO_WRAP)
      val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
      val key = store.getKey(KEY_ALIAS, null) as? SecretKey
      if (key == null) {
        invoke.reject("the Android Keystore key is gone", "missing")
        return
      }
      val cipher = Cipher.getInstance(TRANSFORMATION)
      cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, blob, 0, 12))
      val out = cipher.doFinal(blob, 12, blob.size - 12)
      invoke.resolve(JSObject().put("data", Base64.encodeToString(out, Base64.NO_WRAP)))
    } catch (e: Exception) {
      invoke.reject("Android Keystore: ${e.message}")
    }
  }

  // ----- files -----

  private fun mimeOf(name: String): String {
    val ext = name.substringAfterLast('.', "").lowercase()
    return MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "application/octet-stream"
  }

  private fun contentUri(path: String): Uri =
    FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", File(path))

  /** Opens a file from PeetMail's cache folder in the app the user picks for its type. */
  @Command
  fun openFile(invoke: Invoke) {
    try {
      val args = invoke.getArgs()
      val path = args.getString("path")
      val intent = Intent(Intent.ACTION_VIEW)
        .setDataAndType(contentUri(path), mimeOf(args.getString("name")))
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
      // Not a chooser: Android asks which app when there are several, and with none this
      // throws, so the message below can say so.
      activity.startActivity(intent)
      invoke.resolve()
    } catch (e: android.content.ActivityNotFoundException) {
      invoke.reject("No app on this phone opens this kind of file.", "noApp")
    } catch (e: Exception) {
      invoke.reject(e.message)
    }
  }

  /** Copies a file into Downloads. Returns the name Android gave it (it adds " (1)" on a clash). */
  @Command
  fun saveToDownloads(invoke: Invoke) {
    try {
      val args = invoke.getArgs()
      val name = args.getString("name")
      val resolver = activity.contentResolver
      val values = ContentValues().apply {
        put(MediaStore.Downloads.DISPLAY_NAME, name)
        put(MediaStore.Downloads.MIME_TYPE, mimeOf(name))
        put(MediaStore.Downloads.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS)
        put(MediaStore.Downloads.IS_PENDING, 1)
      }
      val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
        ?: throw IllegalStateException("Android wouldn't create the file in Downloads")
      try {
        resolver.openOutputStream(uri).use { out ->
          File(args.getString("path")).inputStream().use { it.copyTo(out!!) }
        }
        resolver.update(uri, ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }, null, null)
      } catch (e: Exception) {
        resolver.delete(uri, null, null)
        throw e
      }
      invoke.resolve(JSObject().put("name", displayName(uri) ?: name))
    } catch (e: Exception) {
      invoke.reject(e.message)
    }
  }

  /** The share sheet, for a file PeetMail made (the diagnostics zip). */
  @Command
  fun shareFile(invoke: Invoke) {
    try {
      val args = invoke.getArgs()
      val intent = Intent(Intent.ACTION_SEND)
        .setType(mimeOf(args.getString("name")))
        .putExtra(Intent.EXTRA_STREAM, contentUri(args.getString("path")))
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
      activity.startActivity(Intent.createChooser(intent, args.getString("title")))
      invoke.resolve()
    } catch (e: Exception) {
      invoke.reject(e.message)
    }
  }

  /** Android's notification settings for PeetMail (after the permission was refused). */
  @Command
  fun openNotificationSettings(invoke: Invoke) {
    try {
      activity.startActivity(
        Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, activity.packageName)
      )
      invoke.resolve()
    } catch (e: Exception) {
      invoke.reject(e.message)
    }
  }

  // ----- what other apps hand us -----

  /** What arrived since the last call, and forgets it: `shared`, `redirect`, `open`. */
  @Command
  fun takePending(invoke: Invoke) {
    val out: JSObject
    synchronized(pending) {
      out = JSObject(pending.toString())
      pending.keys().asSequence().toList().forEach { pending.remove(it) }
    }
    invoke.resolve(out)
  }

  private fun onIntent(intent: Intent) {
    // The same intent comes back when the activity is recreated (rotation, process restart).
    if (intent.getBooleanExtra(HANDLED, false)) return
    val found: Pair<String, Any>? = try {
      when {
        intent.action == Intent.ACTION_VIEW && intent.data?.scheme == activity.packageName ->
          "redirect" to intent.data.toString()
        intent.action == Intent.ACTION_SEND || intent.action == Intent.ACTION_SEND_MULTIPLE ->
          "shared" to shared(intent)
        intent.hasExtra(NOTIFICATION_KEY) -> notificationTarget(intent)?.let { "open" to it }
        else -> null
      }
    } catch (e: Exception) {
      android.util.Log.w("PeetMail", "couldn't read what was handed to PeetMail", e)
      null
    }
    intent.putExtra(HANDLED, true)
    if (found == null) return
    synchronized(pending) { pending.put(found.first, found.second) }
    webView?.post { webView?.evaluateJavascript("window.dispatchEvent(new Event('peetmail-pending'))", null) }
  }

  /** Text, a subject and files shared to PeetMail. Files are copied into the cache, since the
   *  permission to read them lasts only as long as this activity. */
  private fun shared(intent: Intent): JSObject {
    val uris = mutableListOf<Uri>()
    if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
      intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)?.let { uris.addAll(it) }
    } else {
      intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)?.let { uris.add(it) }
    }
    val dir = File(activity.cacheDir, "shared/${UUID.randomUUID()}").apply { mkdirs() }
    val files = JSArray()
    val failed = JSArray()
    for (uri in uris) {
      var name = uri.lastPathSegment ?: "file"
      try {
        name = (displayName(uri) ?: name).replace('/', '_').ifBlank { "file" }
        val file = File(dir, name)
        val input = activity.contentResolver.openInputStream(uri) ?: throw IllegalStateException("the app that shared it didn't send it")
        input.use { file.outputStream().use { out -> it.copyTo(out) } }
        files.put(JSObject().put("path", file.absolutePath).put("name", name))
      } catch (e: Exception) {
        android.util.Log.w("PeetMail", "couldn't read a shared file", e)
        failed.put("$name: PeetMail wasn't allowed to read it")
      }
    }
    return JSObject()
      .put("text", intent.getStringExtra(Intent.EXTRA_TEXT))
      .put("subject", intent.getStringExtra(Intent.EXTRA_SUBJECT))
      .put("files", files)
      .put("failed", failed)
  }

  /** The conversation a tapped notification was about (src-tauri/src/notify.rs puts it in `extra`). */
  private fun notificationTarget(intent: Intent): JSObject? {
    val extra = JSONObject(intent.getStringExtra(NOTIFICATION_KEY) ?: return null).optJSONObject("extra") ?: return null
    if (!extra.has("conversationId")) return null
    return JSObject().put("accountId", extra.getLong("accountId")).put("conversationId", extra.getString("conversationId"))
  }

  private fun displayName(uri: Uri): String? =
    activity.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
      if (it.moveToFirst()) it.getString(0) else null
    }

  companion object {
    private const val KEY_ALIAS = "peetmail-sign-ins"
    private const val TRANSFORMATION = "AES/GCM/NoPadding"
    private const val HANDLED = "com.peetsoftware.peetmail.handled"
    /** tauri-plugin-notification's extra holding the tapped notification (TauriNotificationManager.kt). */
    private const val NOTIFICATION_KEY = "LocalNotficationObject"
  }
}
