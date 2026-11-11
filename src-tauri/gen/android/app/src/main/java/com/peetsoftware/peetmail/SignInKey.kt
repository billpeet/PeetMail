package com.peetsoftware.peetmail

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * The Keystore key that wraps the key for saved sign-ins (src-tauri/src/secure.rs, `wrapped`).
 * Rust asks for wrap/unwrap through PeetPlugin; the background check (SyncWorker) unwraps the
 * key itself, since Android may start it with no plugin loaded.
 */
object SignInKey {
  private const val KEY_ALIAS = "peetmail-sign-ins"
  private const val TRANSFORMATION = "AES/GCM/NoPadding"
  /** secure.rs `WRAPPED_FILE` and, from before the Keystore, `KEY_FILE`. */
  private const val WRAPPED_FILE = "token.key.wrapped"
  private const val PLAIN_FILE = "token.key"

  private fun existing(): SecretKey? {
    val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    return store.getKey(KEY_ALIAS, null) as? SecretKey
  }

  private fun keystoreKey(): SecretKey {
    existing()?.let { return it }
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

  /** The 12-byte nonce, then the sealed bytes. */
  fun wrap(data: ByteArray): ByteArray {
    val cipher = Cipher.getInstance(TRANSFORMATION)
    cipher.init(Cipher.ENCRYPT_MODE, keystoreKey())
    return cipher.iv + cipher.doFinal(data)
  }

  /** What `wrap` made, or null when the Keystore key is gone. Throws on any other failure. */
  fun unwrap(blob: ByteArray): ByteArray? {
    val key = existing() ?: return null
    val cipher = Cipher.getInstance(TRANSFORMATION)
    cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, blob, 0, 12))
    return cipher.doFinal(blob, 12, blob.size - 12)
  }

  /** The key for saved sign-ins, from the data folder, or null if there's none yet (or it
   *  can't be read: the app sorts that out when it's next opened). */
  fun load(dataDir: File): ByteArray? {
    val wrapped = File(dataDir, WRAPPED_FILE)
    if (wrapped.exists()) return unwrap(wrapped.readBytes())?.takeIf { it.size == 32 }
    // Not opened since the update that moved it into the Keystore: the app moves it.
    return File(dataDir, PLAIN_FILE).takeIf { it.exists() }?.readBytes()?.takeIf { it.size == 32 }
  }
}
