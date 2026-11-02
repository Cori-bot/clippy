# ADR : un dossier de clips qui ne contient que des vidéos

Statut : accepté — 2 novembre 2026.

## Contexte
Chaque clip laissait dans le dossier des clips un `<id>.json` (métadonnées), un `<id>.jpg` (miniature) et un dossier `.clippy-cache\` (pistes audio de l’éditeur). Les captures d’écran avaient en plus un `<id>.thumb.jpg`. Les autres apps ne font pas ça : l’app NVIDIA ne laisse que des `.mp4`, et Medal garde son index dans `%APPDATA%\Medal\store\clips.json`.

## Décision
- **Le dossier des clips ne contient que les vidéos**, rangées par jeu. Elles sont nommées comme un humain le ferait : « VALORANT 2026-11-02 14-27-22.mp4 », ou « Bureau … » hors jeu, avec « (2) » en cas de doublon.
- **Le titre est le nom du fichier.** Renommer un clip dans Clippy renomme le `.mp4`, et c’est refusé si le nom est déjà pris.
- **Index** : `%APPDATA%\clippy\library.json`, à côté de `config.toml` (`clippy_core::store`). Il fait correspondre id → chemin, taille et date de modification, ainsi que ce que le fichier ne dit pas :
  - favori, jeu ;
  - noms des pistes, encodeur ;
  - date de création avec son fuseau.

  L’id reste un horodatage `AAAAMMJJ_HHMMSS[_N]` : le protocole et l’UI gardent leurs clés à travers un renommage.
- **Caches jetables** : `%LOCALAPPDATA%\Clippy\cache\{thumbs,tracks,shots}`.
  - Une miniature absente est remplacée dans la grille par une image de la vidéo.
  - Une piste absente est ré-extraite.
- **Scan** :
  1. Un chemin connu reprend son entrée.
  2. Un fichier inconnu qui a la même taille et la même date de modification qu’une entrée dont le fichier a disparu est ce clip, renommé ou déplacé dans l’Explorateur.
  3. Sinon le fichier est adopté, **seulement si son nom est un nom donné par Clippy**. Le dossier des clips peut être un disque entier partagé avec l’app NVIDIA, et adopter ses vidéos les soumettrait au plafond de taille, qui supprime.
- Les entrées dont le fichier manque ne sont retirées que si le dossier des clips est lisible : un disque débranché ne fait perdre aucun favori.
- **Migration** de l’ancien format au premier scan. On écrit d’abord l’index, ensuite on renomme, et on ne supprime le `.json` qu’une fois le clip classé. On peut donc l’interrompre n’importe où. Un `.json` n’est supprimé que s’il se lit comme un sidecar de Clippy.

## Vérification
- Tests `store::tests` : migration, rattachement, adoption, disque débranché, renommage, noms de fichier, plafond, fichiers d’une autre app laissés intacts.
- Tests du daemon : suppression, renommage, favori, export.
- Tests UI : `clips.test.ts` et `shots.test.ts`.
- Migration rejouée sur une copie de vrais fichiers dans `.test-tmp`.
