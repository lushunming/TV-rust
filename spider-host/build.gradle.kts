import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// settings.gradle.kts 的 pluginManagement 已把 kotlin("jvm") 2.3.0 载入构建类路径，
// 模块内不能再以不同版本请求（会报 already on the classpath）。
// serialization 插件因此走经典 buildscript classpath + apply 方式，版本与 kotlin 保持一致。
buildscript {
    dependencies {
        classpath("org.jetbrains.kotlin:kotlin-serialization:2.3.0")
    }
}

plugins {
    kotlin("jvm")
    application
}

apply(plugin = "org.jetbrains.kotlin.plugin.serialization")

// spider-host: 轻量 JVM 宿主进程。
// 职责：加载爬虫 jar（URLClassLoader），以 stdio JSON-RPC 方式向 Rust UI 提供爬虫调用。
// 不依赖 composeApp / Compose / Room / Ktor，SDK 面以源码副本形式内置（com.github.catvod.* / com.corner.* 工具类）。

dependencies {
    // 与 composeApp 的 "spider depend on" 依赖保持一致，保证既有爬虫 jar 可直接运行
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

    implementation("org.apache.commons:commons-lang3:3.14.0")
    implementation("org.slf4j:slf4j-api:2.0.13")
    implementation("ch.qos.logback:logback-classic:1.3.14")

    testImplementation(kotlin("test"))
}

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
