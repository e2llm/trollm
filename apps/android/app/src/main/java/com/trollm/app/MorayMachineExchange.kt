package com.trollm.app

import org.json.JSONObject
import java.io.File
import java.net.URI
import java.net.URLEncoder
import java.nio.ByteBuffer
import java.nio.charset.StandardCharsets.UTF_8
import java.security.KeyFactory
import java.security.Signature
import java.security.spec.RSAPublicKeySpec
import java.math.BigInteger
import java.time.Instant
import java.util.Base64
import javax.net.ssl.HttpsURLConnection

/** Exact issuer endpoints, platform hostname verification and existing operator CA.
 * No redirects, browser, password, refresh-token grant or automatic HTTP retry.
 */
class MorayMachineHttps(
    private val filesDir: File,
    private val now: () -> Instant = Instant::now,
    totalBudgetMillis: Long = 30_000,
    private val connectionFactory: (String) -> HttpsURLConnection = {
        URI(it).toURL().openConnection() as HttpsURLConnection
    },
) : MorayMachineExchange {
    private val budget = MorayMachineBudget(totalBudgetMillis)

    override fun acquire(profile: MorayProfile, credential: MorayMachineCredential): MorayMachineAccess = budget.run { attempt ->
        var stage = "REQUEST_CONFIGURATION"
        try {
            require(credential.tokenEndpoint == profile.issuer + "/protocol/openid-connect/token")
            val form = linkedMapOf("grant_type" to "client_credentials", "client_id" to credential.clientId,
                "client_secret" to credential.secret, "scope" to MorayMachineClaims.scopes.sorted().joinToString(" "))
                .entries.joinToString("&") { (key, value) ->
                    URLEncoder.encode(key, "UTF-8") + "=" + URLEncoder.encode(value, "UTF-8")
                }.toByteArray(UTF_8)
            stage = "TOKEN_RESPONSE"
            val response = json(request(credential.tokenEndpoint, form, attempt))
            require(response.sourceString("token_type").equals("Bearer", ignoreCase = true))
            require(!response.has("refresh_token") && !response.has("id_token"))
            val lifetime = integer(response, "expires_in")
            require(lifetime in 16L..300L)
            val token = response.sourceString("access_token")
            require(ConnectionInput.validToken(token) && token.length <= 32_768)
            // A new issuer key is accepted only via the exact trusted HTTPS issuer.
            // API-side offline JWKS reload remains a separate installation action.
            stage = "JWKS_RESPONSE"
            val keys = json(request(profile.issuer + "/protocol/openid-connect/certs", null, attempt))
            attempt.check()
            val expires = MorayMachineClaims.verify(token, keys, profile, credential.subject, now())
            MorayMachineAccess(token, expires, lifetime)
        } catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled
        } catch (closed: MorayFailure) { throw closed
        } catch (claims: MorayMachineClaimFailure) { throw MorayFailure(claims.code)
        } catch (_: Exception) { throw MorayFailure("SOURCE_MACHINE_" + stage + "_INVALID") }
    }

    private fun request(url: String, body: ByteArray?, attempt: MorayMachineBudget.Attempt): ByteArray {
        attempt.check()
        val connection = connectionFactory(ConnectionInput.canonicalEndpoint(url))
        try {
            attempt.attach(connection)
            OperatorTrustRoots.socketFactory(File(filesDir, "trollm/trust/moray-roots.pem").toPath())
                ?.let { connection.sslSocketFactory = it }
            connection.instanceFollowRedirects = false
            connection.connectTimeout = attempt.socketTimeoutMillis()
            connection.readTimeout = attempt.socketTimeoutMillis()
            connection.requestMethod = if (body == null) "GET" else "POST"
            connection.setRequestProperty("Accept", "application/json")
            if (body != null) {
                connection.doOutput = true
                connection.setRequestProperty("Content-Type", "application/x-www-form-urlencoded")
                connection.setFixedLengthStreamingMode(body.size)
                attempt.check()
                connection.outputStream.use { output ->
                    attempt.check()
                    output.write(body)
                    attempt.check()
                }
            }
            attempt.check()
            val status = connection.responseCode
            attempt.check()
            if (status != 200) throw MorayFailure("SOURCE_MACHINE_HTTP_$status")
            return connection.inputStream.use { input ->
                val bounded = object : java.io.FilterInputStream(input) {
                    override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
                        attempt.check()
                        val read = input.read(buffer, offset, length)
                        attempt.check()
                        return read
                    }
                }
                BundleInput.read(bounded, 131_072)
            }
        } catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled
        } catch (closed: MorayFailure) { throw closed
        } catch (_: javax.net.ssl.SSLException) { throw MorayFailure("SOURCE_MACHINE_TLS_FAILED")
        } catch (_: java.net.SocketTimeoutException) { throw MorayFailure("SOURCE_MACHINE_TIMEOUT")
        } catch (_: java.io.IOException) { throw MorayFailure("SOURCE_MACHINE_TRANSPORT_UNAVAILABLE")
        } finally { attempt.close(connection) }
    }

    companion object {
        internal fun json(bytes: ByteArray): JSONObject = JSONObject(UTF_8.newDecoder().decode(ByteBuffer.wrap(bytes)).toString())
        internal fun integer(value: JSONObject, name: String): Long {
            val raw = value.get(name)
            require(raw is Number && raw.toString().matches(Regex("[0-9]{1,12}")))
            return raw.toString().toLong()
        }
    }
}

/** Closed classification only: never carries token bytes, claims or a raw cause. */
internal class MorayMachineClaimFailure(val code: String) : IllegalArgumentException(code)

/** Cryptographic issuer identity, not owner registration or business acceptance. */
object MorayMachineClaims {
    val scopes = setOf("moray:bundle:read", "moray:edge-report:write")

    fun verify(token: String, jwks: JSONObject, profile: MorayProfile, subject: String, now: Instant): Instant {
        var stage = "ENCODING"
        try {
            val parts = token.split('.')
            require(parts.size == 3 && parts.all { it.isNotEmpty() && it.matches(Regex("[A-Za-z0-9_-]+")) })
            fun decode(value: String): ByteArray = Base64.getUrlDecoder().decode(value).also {
                require(Base64.getUrlEncoder().withoutPadding().encodeToString(it) == value)
            }
            stage = "HEADER"
            val header = MorayMachineHttps.json(decode(parts[0]))
            require(header.sourceString("alg") == "RS256" && header.sourceString("typ") == "JWT")
            require(JsonValues.keys(header).all { it in setOf("alg", "typ", "kid") })
            stage = "KEY"
            val kid = header.sourceString("kid")
            val entries = jwks.getJSONArray("keys")
            require(entries.length() in 1..32)
            val matches = (0 until entries.length()).map { entries.getJSONObject(it) }.filter { it.opt("kid") == kid }
            require(matches.size == 1)
            val key = matches.single()
            require(key.sourceString("kty") == "RSA" && key.opt("use") == "sig" && key.opt("alg") == "RS256")
            require(!key.has("d"))
            val modulus = BigInteger(1, decode(key.sourceString("n")))
            val exponent = BigInteger(1, decode(key.sourceString("e")))
            require(modulus.bitLength() in 2048..8192 && exponent >= BigInteger.valueOf(3))
            val publicKey = KeyFactory.getInstance("RSA").generatePublic(RSAPublicKeySpec(modulus, exponent))
            stage = "SIGNATURE"
            val verifier = Signature.getInstance("SHA256withRSA")
            verifier.initVerify(publicKey)
            verifier.update((parts[0] + "." + parts[1]).toByteArray(UTF_8))
            require(verifier.verify(decode(parts[2])))
            stage = "BODY"
            val claims = MorayMachineHttps.json(decode(parts[1]))
            stage = "IDENTITY"
            require(claims.sourceString("iss") == profile.issuer && claims.sourceString("azp") == profile.clientId &&
                claims.sourceString("sub") == subject && claims.sourceString("aud") == "urn:moray:portable" &&
                claims.sourceString("typ") == "Bearer")
            stage = "SCOPE"
            val suppliedScopes = claims.sourceString("scope").split(' ')
            require(suppliedScopes.size == scopes.size && suppliedScopes.toSet() == scopes)
            stage = "TIME_FORMAT"
            val issued = Instant.ofEpochSecond(MorayMachineHttps.integer(claims, "iat"))
            val expires = Instant.ofEpochSecond(MorayMachineHttps.integer(claims, "exp"))
            stage = "ISSUED_FUTURE"
            require(issued <= now)
            stage = "EXPIRY_RESERVE"
            require(expires > now.plusSeconds(15))
            stage = "LIFETIME"
            require(expires > issued && expires <= issued.plusSeconds(300))
            if (claims.has("nbf")) {
                stage = "NOT_BEFORE"
                require(Instant.ofEpochSecond(MorayMachineHttps.integer(claims, "nbf")) <= now)
            }
            return expires
        } catch (_: Exception) {
            throw MorayMachineClaimFailure("SOURCE_MACHINE_CLAIMS_" + stage)
        }
    }
}
