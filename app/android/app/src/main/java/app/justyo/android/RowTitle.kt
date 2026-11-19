package app.justyo.android

import uniffi.yo.ListRow
import uniffi.yo.ProductLanguage
import uniffi.yo.ProductText
import uniffi.yo.listBarTitle
import uniffi.yo.productRequestRow
import uniffi.yo.productText

/** Yo's own words come from the core (`yo_product`), in English like iOS. */
internal val Words = ProductLanguage.ENGLISH
internal fun copy(text: ProductText) = productText(text, Words)

/** A bar's title: the core's row title, a request's line, or the name (a group's with its #). */
internal fun rowTitle(row: ListRow?, friend: Friend): String {
    if (friend.request) return productRequestRow(friend.name, friend.group != null, Words)
    val title = row?.title ?: friend.name
    return if (title == friend.name) listBarTitle(title, friend.group != null) else title
}
