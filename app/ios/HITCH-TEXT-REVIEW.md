# Explanation text changes

**AccountInfo.swift**

Keeps your contacts with the names and colours you gave them, the groups you're in, blocked people, your profile key and settings like the app icon and left-handed mode. Not photos, videos or Yos. End-to-end encrypted: sealed on this phone with a key derived from your account key, which never leaves your devices. The server stores it but can't read it.

→ Your contacts, names, colours, groups, blocks, profile key and settings sync between your devices; Yos, photos and videos don’t. Your data is end-to-end encrypted with a key made from your account key. The server stores it but can’t read it.

**AccountInfo.swift**

Your account is a secret key on this phone: your Yo identity. Recovery keeps an end-to-end encrypted copy of it at the server, so a new phone can get it back. The copy can be unlocked with your recovery key or, if on, your passkey using your password manager's authentication, such as Face ID; the server never sees either secret. iCloud Keychain also keeps your account, end-to-end encrypted by Apple. Turn on a passkey or a recovery key so you don't depend on iCloud alone.

→ Your account key identifies you on Yo; recovery stores an end-to-end encrypted copy on the server. You can restore it with your recovery key or an enabled passkey after your password manager checks it’s you; the server can’t read your key or either secret. iCloud Keychain also keeps your account with Apple’s end-to-end encryption, so add a passkey or recovery key for another way back in.

**HelpPage.swift**

Sends anonymized crash reports and diagnostics. No names, no messages.

→ You share crash reports with the Yo team. Reports contain no names, Yos or photos.

**Blocked.swift**

On, nobody new can send you a Yo Request. Friends you have keep working.

→ You won’t receive requests from new people. Your friends can still send you Yos.

**Blocked.swift**

Blocked friends can’t send you Yos. Swipe to unblock.

→ You won’t receive Yos from blocked friends. Swipe a name to unblock them.

**StatusPage.swift**

Data Use counts upload and download payloads from this build onward; its directional detail is for this month.

→ You can see data sent and received since this version was installed. Upload and download totals are shown for this month.

**Contacts.swift**

The keys others use to start encrypted chats with you. Yo signs them, keeps enough at the hub and renews them on its own. Only public key material is shown.

→ Your friends use these public keys to start encrypted chats with you. Yo signs and renews them automatically. Your private keys aren’t shown.

**Contacts.swift**

A Yo to \(friend.name) waits \(seconds) seconds with a ring to undo it, as a photo does.

→ Your Yo waits \(seconds) seconds before sending. Tap the ring to cancel.

**Contacts.swift**

Compare safety numbers in person or on a call before verifying. If their key changes, Yos wait for your confirmation.

→ Compare your safety numbers in person or on a call, then mark this friend as verified. If their key changes, your Yos wait until you confirm.

**Contacts.swift**

Their key changed: they may have a new iPhone. To be sure it’s them, compare safety numbers.

→ Your friend’s key changed, possibly on a new phone. Compare safety numbers to check it’s still them.

**Contacts.swift**

Their key changed after you verified them. Compare safety numbers again.

→ Your friend’s key changed after you verified them. Compare safety numbers again.

**Contacts.swift**

\(friend.name) uses an older Yo. Your Yos wait and arrive once they update the app.

→ Your Yos wait until \(friend.name) updates Yo.

**Intro.swift**

That’s a Yo. Nothing to type.

→ Tap a friend’s name to send a Yo.

**Intro.swift**

A Yo comes in. Tap their name to Yo back.

→ When you receive a Yo, tap the sender’s name to reply.

**Intro.swift**

YoGPT is there from the start. It Yos back.

→ You can try Yo with YoGPT, your first contact.

**YoApp.swift**

Send a short crash report to the Yo team? It holds no Yos, names or photos and is not linked to you.

→ Share a crash report with the Yo team? It contains no Yos, names or photos and isn’t linked to your account.

**NewGroup.swift**

Up to \(Self.most) friends.

→ You can add up to \(Self.most) friends, plus yourself.

**GroupNotice.swift**

They won’t get the group’s Yos any more.

→ They won’t receive any more Yos from this group.
