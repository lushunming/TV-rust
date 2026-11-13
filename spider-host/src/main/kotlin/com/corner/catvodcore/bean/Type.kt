package com.corner.catvodcore.bean

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.Transient

@Serializable
class Type {
    constructor(id: String, name: String) {
        this.typeId = id
        this.typeName = name
    }

    @SerialName("type_id")
    var typeId: String = ""

    @SerialName("type_name")
    var typeName: String = ""

    @Transient
    var selected: Boolean = false

    @Transient
    var failTime: Int = 0

    companion object {
        fun home(): Type {
            val type = Type("home", "推荐")
            type.selected = true
            return type
        }
    }
}
