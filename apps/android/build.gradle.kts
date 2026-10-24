// AGP 8.8.2 supports API 35 and Gradle 8.11.1; see README.md for validation limits.
plugins {
    id("com.android.application") version "8.8.2" apply false
    id("org.jetbrains.kotlin.android") version "2.1.20" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.1.20" apply false
}

// CI and constrained developer machines may keep generated output outside the
// source checkout (for example on a larger secondary drive).
providers.gradleProperty("trollm.buildRoot").orNull?.let { root ->
    subprojects {
        layout.buildDirectory.set(rootProject.file(root).resolve(name))
    }
}
