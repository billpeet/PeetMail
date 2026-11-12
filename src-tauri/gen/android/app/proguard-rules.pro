# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile
# The background check calls into Rust by this name (src-tauri/src/background.rs).
-keepclasseswithmembernames class com.peetsoftware.peetmail.SyncWorker$Companion { native <methods>; }
-keepclasseswithmembernames class com.peetsoftware.peetmail.SyncWorker { native <methods>; }

# WorkManager makes these by reflection. Its own keep rules should cover them, but the first
# release build (AGP 9, R8) dropped WorkDatabase_Impl's constructor and crashed at startup.
-keep class * extends androidx.room.RoomDatabase { <init>(); }
-keep class * extends androidx.work.ListenableWorker { <init>(android.content.Context, androidx.work.WorkerParameters); }
-keep class * extends androidx.work.InputMerger { <init>(); }
