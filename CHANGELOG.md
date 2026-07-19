# Changelog

## [0.5.0](https://github.com/m1sk9/HoneyPot/compare/honeypot-v0.4.1...honeypot-v0.5.0) (2026-07-19)


### Features

* add slash command system (/help, /version, /ping, /whois, /doctor) ([#33](https://github.com/m1sk9/HoneyPot/issues/33)) ([801d877](https://github.com/m1sk9/HoneyPot/commit/801d8778d2778583b41680c0e9fdd7302bd81341))

## [0.4.1](https://github.com/m1sk9/HoneyPot/compare/honeypot-v0.4.0...honeypot-v0.4.1) (2026-07-19)


### Miscellaneous

* **deps:** update rust crate serde to v1.0.229 ([#31](https://github.com/m1sk9/HoneyPot/issues/31)) ([8cde9c8](https://github.com/m1sk9/HoneyPot/commit/8cde9c8000e0f3bc209a17693e2435fd2863299f))
* **deps:** update rust crate thiserror to v2.0.19 ([#32](https://github.com/m1sk9/HoneyPot/issues/32)) ([7ec76c1](https://github.com/m1sk9/HoneyPot/commit/7ec76c190b24e738717a35d332e07982535458ff))
* **deps:** update rust crate tokio to v1.52.4 ([#28](https://github.com/m1sk9/HoneyPot/issues/28)) ([784d2ca](https://github.com/m1sk9/HoneyPot/commit/784d2ca422de0a6169e892fab35e18e017b87514))
* **deps:** update rust crate tokio to v1.53.0 ([#30](https://github.com/m1sk9/HoneyPot/issues/30)) ([84a5c5d](https://github.com/m1sk9/HoneyPot/commit/84a5c5d39249d106d060a0b1b9bad5c9dfc1d13c))

## [0.4.0](https://github.com/m1sk9/HoneyPot/compare/honeypot-v0.3.0...honeypot-v0.4.0) (2026-07-15)


### Features

* fire role trigger only on self-assigned honeypot roles ([#25](https://github.com/m1sk9/HoneyPot/issues/25)) ([ce75ed3](https://github.com/m1sk9/HoneyPot/commit/ce75ed34b94ad6bfa9a4ed9086ff60d09967dcd4))


### Miscellaneous

* **deps:** update rust crate toml to v1.1.3 ([#27](https://github.com/m1sk9/HoneyPot/issues/27)) ([59f3c81](https://github.com/m1sk9/HoneyPot/commit/59f3c81b26729edbae4396712aa747169434b593))

## [0.3.0](https://github.com/m1sk9/HoneyPot/compare/honeypot-v0.2.0...honeypot-v0.3.0) (2026-07-07)


### ⚠ BREAKING CHANGES

* log the offending message on channel-triggered honeypot bans ([#19](https://github.com/m1sk9/HoneyPot/issues/19))

### Features

* add dry-run mode to simulate bans without executing them ([#20](https://github.com/m1sk9/HoneyPot/issues/20)) ([a7293e3](https://github.com/m1sk9/HoneyPot/commit/a7293e337f047bb73de592abc0242d70fd25325d))
* enrich honeypot ban log embed with account signals ([#21](https://github.com/m1sk9/HoneyPot/issues/21)) ([83231d4](https://github.com/m1sk9/HoneyPot/commit/83231d44fa01af63827add0d0f779df80499859b))
* localize log embeds and button responses per guild (en/ja) ([#22](https://github.com/m1sk9/HoneyPot/issues/22)) ([1b07774](https://github.com/m1sk9/HoneyPot/commit/1b0777459068ec39eaf4ae2ac2d05a78424115d6))
* log the offending message on channel-triggered honeypot bans ([#19](https://github.com/m1sk9/HoneyPot/issues/19)) ([81c2b4d](https://github.com/m1sk9/HoneyPot/commit/81c2b4d1ae32ed31b6ce8bb69b3d8ee78e4fd3eb))
* require confirmation before unban and manual ban actions ([#17](https://github.com/m1sk9/HoneyPot/issues/17)) ([ce16b20](https://github.com/m1sk9/HoneyPot/commit/ce16b20136eb0dca584635bfae899ea19dcf6569))

## [0.2.0](https://github.com/m1sk9/HoneyPot/compare/honeypot-v0.1.0...honeypot-v0.2.0) (2026-07-03)


### Features

* defer honeypot bans for untrusted bots to manual review ([#14](https://github.com/m1sk9/HoneyPot/issues/14)) ([4da4054](https://github.com/m1sk9/HoneyPot/commit/4da4054828075a609a4121d8531e518511ececcd))

## 0.1.0 (2026-07-02)


### Features

* auto-ban honeypot triggers with log notification ([#9](https://github.com/m1sk9/HoneyPot/issues/9)) ([7b50dd0](https://github.com/m1sk9/HoneyPot/commit/7b50dd07cae1378663dc7cf3eef52d7dc675e6d5))
* load environment variables from a .env file ([#10](https://github.com/m1sk9/HoneyPot/issues/10)) ([c5d4e29](https://github.com/m1sk9/HoneyPot/commit/c5d4e29260f0e8837d6b38fe73fc5caba8a7a0eb))
* project foundation and CI (config, client bootstrap) ([e5c6ee4](https://github.com/m1sk9/HoneyPot/commit/e5c6ee41970262715c2e199d4e0398f8b49317e0))
* project foundation and CI (config, client bootstrap) ([6aa8e91](https://github.com/m1sk9/HoneyPot/commit/6aa8e913bed27a8e1006e55b5c7bc84fb6057976))
* unban members via the log notification button ([#11](https://github.com/m1sk9/HoneyPot/issues/11)) ([8e8a691](https://github.com/m1sk9/HoneyPot/commit/8e8a691de5d6fb3d2c89a5d0a1782d10e565a236))


### Miscellaneous

* pin dependency versions to full semver ([551f5de](https://github.com/m1sk9/HoneyPot/commit/551f5de735d48c5e6811e600119838117ffeded3))
* set initial version to 0.0.0 for release-please ([e26fa85](https://github.com/m1sk9/HoneyPot/commit/e26fa85b65515983fd65326c04f6cdccc5e15cce))
