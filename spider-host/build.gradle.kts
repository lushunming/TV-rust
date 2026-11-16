import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// spider-host: 轻量 JVM 宿主进程，**独立 Gradle 工程**（settings.gradle.kts 在本目录）。
// 职责：加载爬虫 jar（URLClassLoader），以 stdio JSON-RPC 方式向 Rust UI 提供爬虫调用。
// 不依赖 composeApp / Compose / Room / Ktor，SDK 面以源码副本形式内置（com.github.catvod.* / com.corner.* 工具类）。
// 插件版本必须在这里显式声明——不再有仓库根的 pluginManagement 兜底。
plugins {
    kotlin("jvm") version "2.3.20"
    kotlin("plugin.serialization") version "2.3.20"
    application
}

repositories {
    mavenCentral()
}

dependencies {
    // SDK 面依赖：用 api 而非 implementation —— 爬虫 jar 通过 URLClassLoader 以宿主为父加载器，
    // 编译期也算“对外契约”，后续若拆出 SDK 模块可直接继承。
    api("org.jetbrains.kotlinx:kotlinx-serialization-json:1.7.3")
    // 部分爬虫（如 BaiDuPan）直接使用协程，宿主必须提供
    api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.8.1")
    api(platform("com.squareup.okhttp3:okhttp-bom:5.0.0-alpha.14"))
    api("com.squareup.okhttp3:okhttp")
    api("com.squareup.okhttp3:okhttp-dnsoverhttps")
    api("com.google.code.gson:gson:2.10.1")
    api("org.json:json:20231013")
    api("com.github.lookfirst:sardine:5.10")
    api("cn.wanghaomiao:JsoupXpath:2.5.1")
    api("org.jsoup:jsoup:1.15.3")
    api("com.google.zxing:core:3.3.0")
    api("org.nanohttpd:nanohttpd:2.3.1")
    api("com.google.guava:guava:31.1-jre")
    api("cn.hutool:hutool-all:5.8.27")
    // 部分网盘站（如「电影云集」）用 zstd 解压分享页数据，缺这个类会直接
    // NoClassDefFoundError: com/github/luben/zstd/ZstdException —— 属于宿主该提供的运行时依赖。
    api("com.github.luben:zstd-jni:1.5.7-4")

    implementation("org.apache.commons:commons-lang3:3.14.0")
    implementation("org.slf4j:slf4j-api:2.0.13")
    implementation("ch.qos.logback:logback-classic:1.3.14")

    testImplementation(kotlin("test"))
}

// 字节码目标保持 17：Kotlin 2.3.20 最高只能产出 JVM 25，且爬虫 jar 生态普遍按 17 编译。
// 「JDK 25」升级落在运行时（jlink 镜像 / 启动参数），不在字节码版本。
kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

application {
    mainClass.set("com.corner.host.MainKt")
}

tasks.test {
    useJUnitPlatform()
}

// 产出可被 `java -jar` 直接启动的 fat jar（供 Rust 侧拉起子进程）
tasks.register<Jar>("fatJar") {
    archiveBaseName.set("spider-host")
    archiveClassifier.set("all")
    duplicatesStrategy = DuplicatesStrategy.EXCLUDE
    manifest {
        attributes["Main-Class"] = "com.corner.host.MainKt"
        attributes["Enable-Native-Access"] = "ALL-UNNAMED"
    }
    exclude(
        "META-INF/*.SF",
        "META-INF/*.DSA",
        "META-INF/*.RSA",
        "META-INF/versions/**/module-info.class",
        "module-info.class",
    )
    from(sourceSets.main.get().output)
    from(configurations.runtimeClasspath.get().map { if (it.isDirectory) it else zipTree(it) })
}
