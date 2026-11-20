# Android application libraries

This Maven snapshot contains the Android application libraries and their Maven
metadata. Artifacts retain their published coordinates and bytes. `SHA256SUMS`
records their hashes. The POM files and archives retain upstream license metadata
and notices.

| Directory | Original Maven repository |
| --- | --- |
| `google` | [Google Maven](https://dl.google.com/dl/android/maven2/) |
| `central` | [Maven Central](https://repo.maven.apache.org/maven2/) |

The build environment supplies the JDK, Gradle, Android SDK, build tools and NDK.
Setup stages the Android Gradle Plugin, lint and their libraries in the external
Maven cache described by [`toolchain-maven.sha256`](../../toolchain-maven.sha256).
