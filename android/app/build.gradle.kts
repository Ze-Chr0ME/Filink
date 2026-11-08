plugins { id("com.android.application") }
android {
    namespace = "com.droidtransfer"
    compileSdk = 35
    defaultConfig { applicationId = "com.droidtransfer"; minSdk = 29; targetSdk = 35; versionCode = 1; versionName = "1.0" }
    signingConfigs.getByName("debug") { storeFile = file("E:/Android/home/debug.keystore") }
    buildTypes { release { isMinifyEnabled = true; isShrinkResources = true; signingConfig = signingConfigs.getByName("debug"); proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro") } }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
}
kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) } }
dependencies { testImplementation("junit:junit:4.13.2") }
