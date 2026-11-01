# Clips automatiques

Réglage `auto_clip` (désactivé par défaut) et `auto_clip_min_kills` (1 à 5). Le replay doit être actif.
Le service (`crates/clippy-daemon/src/autoclip.rs`) regroupe les moments rapprochés en une rafale :
le clip est enregistré 3 s après le dernier kill, au plus tard `replay_seconds - 5 s` après le premier.
Il est ensuite renommé, par exemple « Triple kill · Counter-Strike 2 ».

Aucune source ne lit la mémoire d'un jeu.

| Jeu | Source | Détail |
|---|---|---|
| Counter-Strike 2 | Game State Integration (Valve, officiel) | `gamestate_integration_clippy.cfg` est écrit dans `…/game/csgo/cfg` de chaque bibliothèque Steam (registre `SteamPath` + `libraryfolders.vdf`). CS2 le lit **au lancement**. POST JSON vers `http://127.0.0.1:52417/`, token `clippy-autoclip`. Requête bornée à 3 s au total (chaque lecture), 64 en-têtes de 8 Ko max ; toute requête avec `Origin` (navigateur) est refusée. Seuls les kills du joueur local comptent (`player.steamid == provider.steamid`), via la hausse de `round_kills`. Ace au passage à 5, une seule fois. |
| League of Legends | Live Client Data API (Riot, officiel) | Interrogation de `https://127.0.0.1:2999/liveclientdata/` toutes les secondes en partie, toutes les 5 s sinon. Certificat auto-signé non vérifié (loopback). `ChampionKill`, `Multikill` (Double à Penta) et `Ace` du joueur actif, dédupliqués par `EventID`. |
| Valorant | Image capturée (kill feed) | Riot ne fournit pas de flux local. `capture/probe.rs` relit la région du kill feed (x 70–100 %, y 7–32 %) toutes les 200 ms, sans bloquer la capture. Le joueur local a un portrait encadré de jaune : à gauche d'une ligne (côté allié teal), c'est un kill ; à droite, une mort. Le liseré doré de l'écran de mort et les icônes d'assist sont exclus. Chaque ligne a une signature (position du cadre, largeur de la barre) : elle compte une fois, après deux lectures consécutives, et reste mémorisée 5 s après sa disparition (clignotement, effets). Actif seulement quand `valorant-win64-shipping.exe` est le jeu détecté. |

## Vérification

- Tests unitaires : debouncer, tracker CS2, tracker LoL, serveur HTTP, vdf, détecteur et compteur Valorant.
- Opt-in, avec le jeu installé :
  - `cargo test -p clippy-daemon real_cs2 -- --ignored` : trouve le dossier cfg réel.
  - `real_valorant_feed_regions` : régions extraites de vrais enregistrements dans `.test-tmp/valo`.
- Prototype Valorant validé sur des enregistrements réels (E:\Valorant) : kills justes, assists et kills des coéquipiers ignorés.
- **Non validé en partie réelle** : à faire avec CS2, LoL et Valorant lancés.
