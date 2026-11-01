# Journal de diagnostic

Réglage `diagnostics` (Paramètres → Application → Journal de diagnostic), désactivé par défaut. Il s'applique en moins de 2 s, sans redémarrage. Tout est écrit dans `%APPDATA%\clippy\logs` et rien ne quitte le PC. Les fichiers de plus de 14 jours sont supprimés au démarrage du daemon.

| Fichier | Écrit par | Contenu |
|---|---|---|
| `daemon-AAAAMMJJ.log` | daemon (`clippy-daemon/src/diag.rs`) | Journal `tracing` au niveau debug, quel que soit le filtre console, avec le nom du thread. Les plantages y sont toujours écrits, même réglage coupé. |
| `metrics-AAAAMMJJ.jsonl` | daemon, toutes les 2 s | `daemon`, `ui` (clippy-ui et ses processus WebView2) et `foreground` (le jeu) : CPU %, working set, mémoire privée, handles. `system` : CPU %, mémoire, espace libre du disque des clips. `gpu` : moteurs (3D, VideoEncode…) au total, pour le daemon et pour le premier plan, et VRAM. `engine` : fps, images, perdues, cadencées, tampon. `status` : statut du daemon. |
| `system-AAAAMMJJ_HHMMSS.json` | daemon, à chaque activation | Version de Windows, CPU, RAM, GPU (pilote, VRAM), écrans (définition, fréquence), configuration complète, statut. |
| `ui-AAAAMMJJ.jsonl` | fenêtre (`web/src/lib/diag.ts` → commande Tauri `diag_append`) | Clics, double-clics et clics droits (nom du contrôle), touches (raccourcis ; dans un champ texte, seulement qu'une touche a été tapée), changements de réglage, page et clip affichés, toasts, appels au daemon (durée, résultat, erreur), événements du daemon, erreurs JS, avertissements console, tâches longues, erreurs et blocages vidéo. Toutes les 10 s : fps, pire image, tas JS, nombre de nœuds DOM. |

Analyse rapide :

```sh
python -c "import json,collections,glob; L=[json.loads(l) for f in glob.glob(r'%APPDATA%\clippy\logs\ui-*.jsonl') for l in open(f,encoding='utf8')]; print(collections.Counter(x['kind'] for x in L))"
```
