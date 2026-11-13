package com.corner.catvod.enum.bean

import com.corner.catvodcore.bean.Episode
import com.corner.catvodcore.bean.Flag
import com.corner.util.Constants
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.Transient

// 自包含副本：剥离 History / 数据库依赖，其余与原应用保持一致
@Serializable
data class Vod(
    @SerialName("vod_id") var vodId: String = "",
    @SerialName("vod_name") var vodName: String? = null,
    @SerialName("type_name") var typeName: String? = null,
    @SerialName("vod_pic") var vodPic: String? = null,
    @SerialName("vod_remarks") var vodRemarks: String? = null,
    @SerialName("vod_year") var vodYear: String? = null,
    @SerialName("vod_area") var vodArea: String? = null,
    @SerialName("vod_director") var vodDirector: String? = null,
    @SerialName("vod_actor") var vodActor: String? = null,
    @SerialName("vod_content") var vodContent: String? = null,
    @SerialName("vod_play_from") var vodPlayFrom: String? = null,
    @SerialName("vod_play_url") var vodPlayUrl: String? = null,
    @SerialName("vod_tag") var vodTag: String? = null,
    @SerialName("cate") var cate: String? = null,
    @SerialName("style") var style: String? = null,
    @SerialName("land") var land: String? = null,
    @SerialName("circle") var circle: String? = null,
    @SerialName("ratio") var ratio: String? = null,
    @Transient
    var vodFlags: MutableList<Flag> = mutableListOf(),
    @Transient
    var site: Site? = null,
    @Transient
    var currentFlag: Flag = Flag(),
    @Transient
    var subEpisode: MutableList<Episode> = mutableListOf(),
    @Transient
    var currentTabIndex: Int = 0,
) {
    companion object {

        fun Vod.getEpisode(): Episode? {
            return subEpisode.find { it.activated }
        }

        fun Vod.isEmpty(): Boolean {
            return org.apache.commons.lang3.StringUtils.isBlank(vodId) || vodFlags.isEmpty()
        }

        fun Vod.setCurrentFlag(idx: Int) {
            if (vodFlags.isEmpty()) return
            val flag = vodFlags[idx]
            if (flag != null) {
                currentFlag = vodFlags[idx]
                currentFlag.activated = true
            }
        }

        fun Vod.setCurrentFlag(flag: Flag) {
            currentFlag = flag
            currentFlag.activated = true
        }

        fun Vod.setVodFlags() {
            val playFlags: List<String>? = vodPlayFrom?.split("\\$\\$\\$".toRegex())
            val playUrls: List<String>? = vodPlayUrl?.split("\\$\\$\\$".toRegex())

            if (!playFlags.isNullOrEmpty() && !playUrls.isNullOrEmpty()) {
                for (i in 0 until playFlags.size) {
                    if (playFlags[i].isEmpty() || i >= playUrls.size) continue
                    val item = Flag.create(playFlags[i].trim())
                    item.createEpisode(playUrls[i])
                    vodFlags.add(item)
                }
            }
            for (item in vodFlags) {
                if (item.urls == null) continue
                item.createEpisode(item.urls)
            }
            setCurrentFlag(0)
        }

        fun List<Episode>.getPage(index: Int): MutableList<Episode> {
            if (index < 0) return mutableListOf()
            val from = index * Constants.EpSize
            if (from >= size) return mutableListOf()
            val to = minOf(from + Constants.EpSize, size)
            return subList(from, to).toMutableList()
        }
    }

    fun isFolder(): Boolean {
        return VodTag.Folder.called == vodTag
    }

    fun nextFlag(): Flag? {
        val find = vodFlags.find { it.activated }
        val indexOf = vodFlags.indexOf(find)
        if (indexOf + 1 >= vodFlags.size) return null
        val flag = vodFlags[indexOf + 1]
        vodFlags.forEach {
            it.activated = flag.flag == it.flag
        }
        return flag
    }
}

enum class VodTag(val called: String) {
    Folder("folder"),
    File("file")
}
