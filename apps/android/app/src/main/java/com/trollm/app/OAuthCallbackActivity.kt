package com.trollm.app

import android.content.Intent
import android.os.Bundle
import android.widget.TextView
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** AppAuth receives the browser URI; this private activity receives its verified result. */
class OAuthCallbackActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(TextView(this).apply { text = "Completing sign-in…" })
        lifecycleScope.launch {
            val succeeded = try {
                val oauth = withContext(Dispatchers.IO) { OAuthConnections.get(applicationContext) }
                oauth.complete(intent)
            } catch (error: CancellationException) { throw error }
            catch (_: Exception) { false }
            Toast.makeText(this@OAuthCallbackActivity,
                if (succeeded) "Signed in." else "Sign-in did not complete. Try again from Connections.",
                Toast.LENGTH_LONG).show()
            startActivity(Intent(this@OAuthCallbackActivity, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP))
            finish()
        }
    }
}
