# Agents

- The repository is public: never commit secrets, keys, device ids, addresses or personal names. Machine settings go
  in ignored files (`app/ios/dev/local.env`).
- No AI attribution in commits.

## iOS (`app/ios`)

- SwiftUI only, iOS 26, Swift 6. Use the system's own parts wherever iOS has one (NavigationStack, toolbar, its
  Liquid Glass buttons, sheets, search, menus) and keep the Yo look (Flat UI rows, Montserrat Bold, Amethyst) to
  the content. Build your own control only where iOS has none.
- Build on Linux with xtool, install with `app/ios/dev/push.sh` (no cable, no sudo); see `app/ios/README.md`.
