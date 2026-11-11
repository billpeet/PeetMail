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
