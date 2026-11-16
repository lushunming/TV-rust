// spider-host 独立工程配置。
// 以前它作为仓库根工程的一个子模块构建，插件版本、仓库、依赖约束都由根的
// settings.gradle.kts / pluginManagement 提供；现在 spider-host 单独成项目，
// 这些信息必须自包含，否则 `gradlew fatJar` 会报 plugin not found。
pluginManagement {
    repositories {
        gradlePluginPortal()
        mavenCentral()
    }
}

rootProject.name = "spider-host"
