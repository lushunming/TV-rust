// spider-host 独立工程配置。
// 以前它作为仓库根工程的一个子模块构建，插件版本、仓库、依赖约束都由根的
// settings.gradle.kts / pluginManagement 提供；现在 spider-host 单独成项目，
// 这些信息必须自包含，否则 `gradlew fatJar` 会报 plugin not found。
pluginManagement {
    repositories {
        // 阿里云公共代理库（推荐，速度快且稳定）
        maven { url = uri("https://maven.aliyun.com/repository/public") }
        // 阿里云 Google 镜像
        maven { url = uri("https://maven.aliyun.com/repository/google") }
        // 阿里云 Gradle 插件镜像
        maven { url = uri("https://maven.aliyun.com/repository/gradle-plugin") }

        // 保留原有官方源作为备用，防止国内镜像缺少某些冷门插件
        gradlePluginPortal()
        mavenCentral()
    }
}

rootProject.name = "spider-host"