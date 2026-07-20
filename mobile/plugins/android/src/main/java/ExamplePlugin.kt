package cn.crain.codeg.securevault

import android.app.Activity
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import org.json.JSONObject

@InvokeArg
class SecretKeyArgs {
    lateinit var key: String
}

@InvokeArg
class SecretValueArgs {
    lateinit var key: String
    lateinit var value: String
}

@TauriPlugin
class SecureVaultPlugin(private val activity: Activity) : Plugin(activity) {
    private val keyAlias = "cn.crain.codeg.mobile.token-key.v1"
    private val preferences by lazy {
        activity.getSharedPreferences("codeg-secure-vault", Activity.MODE_PRIVATE)
    }

    private fun encryptionKey(): SecretKey {
        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val existing = keyStore.getKey(keyAlias, null)
        if (existing is SecretKey) return existing

        val generator = KeyGenerator.getInstance(
            KeyProperties.KEY_ALGORITHM_AES,
            "AndroidKeyStore"
        )
        generator.init(
            KeyGenParameterSpec.Builder(
                keyAlias,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true)
                .build()
        )
        return generator.generateKey()
    }

    @Command
    fun storeSecret(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(SecretValueArgs::class.java)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, encryptionKey())
            val ciphertext = cipher.doFinal(args.value.toByteArray(Charsets.UTF_8))
            val encoded = listOf(cipher.iv, ciphertext).joinToString(".") {
                Base64.encodeToString(it, Base64.NO_WRAP)
            }
            preferences.edit().putString(args.key, encoded).apply()
            invoke.resolve()
        } catch (error: Exception) {
            invoke.reject("Secure storage failed: ${error.message}")
        }
    }

    @Command
    fun loadSecret(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(SecretKeyArgs::class.java)
            val encoded = preferences.getString(args.key, null)
            val response = JSObject()
            if (encoded == null) {
                response.put("value", JSONObject.NULL)
                invoke.resolve(response)
                return
            }

            val parts = encoded.split(".", limit = 2)
            require(parts.size == 2) { "Invalid encrypted value" }
            val iv = Base64.decode(parts[0], Base64.NO_WRAP)
            val ciphertext = Base64.decode(parts[1], Base64.NO_WRAP)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, encryptionKey(), GCMParameterSpec(128, iv))
            val plaintext = cipher.doFinal(ciphertext).toString(Charsets.UTF_8)
            response.put("value", plaintext)
            invoke.resolve(response)
        } catch (error: Exception) {
            invoke.reject("Secure storage failed: ${error.message}")
        }
    }

    @Command
    fun deleteSecret(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(SecretKeyArgs::class.java)
            preferences.edit().remove(args.key).apply()
            invoke.resolve()
        } catch (error: Exception) {
            invoke.reject("Secure storage failed: ${error.message}")
        }
    }
}
