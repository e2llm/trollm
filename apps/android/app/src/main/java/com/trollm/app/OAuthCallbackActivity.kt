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
            val message = try {
                val oauth = withContext(Dispatchers.IO) { if (intent.getBooleanExtra(OAuthConnections.EXTRA_SOURCE, false))
                    OAuthConnections.moray(applicationContext) else OAuthConnections.get(applicationContext) }
                if (oauth.complete(intent)) "Signed in." else OAuthProblem.FAILED.message
            } catch (error: CancellationException) { throw error }
            catch (error: Exception) { classifyOAuthProblem(error).message }
            Toast.makeText(this@OAuthCallbackActivity, message,
                Toast.LENGTH_LONG).show()
            startActivity(Intent(this@OAuthCallbackActivity, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP))
            finish()
        }
    }
}
