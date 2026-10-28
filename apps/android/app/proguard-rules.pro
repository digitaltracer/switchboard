# R8 rules for release builds. kotlinx.serialization, OkHttp, Compose and ML Kit ship their own.

# The code scanner crashes when started from a minified build (play-services-code-scanner 16.1.0).
-keep class com.google.mlkit.** { *; }
-keep class com.google.android.gms.internal.mlkit_code_scanner.** { *; }
