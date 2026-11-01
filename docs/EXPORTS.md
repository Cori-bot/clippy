# Sorties vidéo et montage

Clippy conserve un **MP4 maître unique** par capture. La première piste audio est le mix audible ; les pistes suivantes gardent les sources pour le montage. Les fichiers de préécoute sont des dérivés reconstruisibles, rangés dans `.clippy-cache`, pas des vidéos supplémentaires.

## Deux sorties

- **MP4 de partage** : une vidéo avec une seule piste audio mixée. Les gains et sourdines de l’éditeur sont appliqués. Si toutes les sources sont coupées, le fichier ne contient volontairement aucune piste audio.
- **MP4 avec pistes audio** : une copie découpée qui conserve le mix et les sources éditables. Sans réglage audio, les pistes sont copiées ; avec réglages, l’audio est reconstruit.

L’original est toujours conservé. Le mode **précis**, sélectionné par défaut dans l’éditeur, coupe sur les timestamps réels des images et réencode la vidéo en H.264. Le mode **rapide** copie la vidéo et aligne le début sur l’image clé précédente. Le recadrage et l’assemblage ne sont pas encore disponibles.

## Partage : taille maximale et presse-papiers

Le sélecteur **Taille du fichier** propose la qualité d’origine ou un plafond adapté à Discord : 20 Mo (compte gratuit depuis août 2026), 50 Mo (Nitro Basic) et 500 Mo (Nitro). Un plafond impose la coupe précise et le MP4 de partage : un seul mix, aucune source séparée.

Le moteur (`export::share_plan`) vise 90 % du plafond. Il retire 192 kb/s par piste AAC, borne le débit vidéo au débit source et refuse sous 150 kb/s, avec la durée maximale dans le message. Sous 0,04 bit par pixel et par image, la définition descend de palier en palier (1080, 720, 540, 480, 360 de petit côté), sans jamais agrandir, en gardant le ratio et des dimensions paires. Si le fichier dépasse malgré tout, le débit est réduit proportionnellement et l’encodage relancé, deux fois au plus, puis l’export échoue sans laisser de fichier.

La réduction d’image se fait image par image sur le NV12 décodé (bilinéaire, Rust). Le processeur vidéo du lecteur Media Foundation n’est pas utilisé : il recale les sources à fréquence variable sur leur fréquence nominale et perd les timestamps réels sur lesquels repose la coupe précise. Les échantillons décodés sans image (flux ouvert avant une image IDR) sont ignorés.

**Copier le fichier** (menu d’un clip, menu de l’éditeur, `Ctrl+C` dans la grille, bouton « Copier pour Discord » après un export) place le MP4 dans le presse-papiers au format fichier (`CF_HDROP`), comme la copie de l’Explorateur. On colle ensuite avec `Ctrl+V` dans Discord, un navigateur ou un dossier.

## Coupe et navigation précises

Les boutons image précédente/suivante et les raccourcis virgule/point mettent en pause et naviguent dans l’index réel des images. I et O marquent la position du lecteur ; la sortie est exclusive. Les poignées se déplacent image par image au clavier en mode précis. Les temps affichent les millisecondes. Un index indisponible désactive l’export précis et propose de réessayer ; le mode rapide reste un choix explicite.

Le moteur rapproche les millisecondes demandées de la borne réelle la plus proche, avec égalité vers la borne antérieure. La borne de fin de la dernière image est disponible pour la sortie uniquement. Une sélection précise non vide peut ne contenir qu’une image ; le mode rapide garde son minimum de 200 ms. Le PCM est rogné à l’échantillon avant mixage. L’AAC peut ajouter son padding d’encodage.

Le réencodage ajoute une génération de compression et utilise actuellement Media Foundation sans accélération GPU imposée. Le décodage vidéo part du début du fichier et le remix audio est préparé en mémoire : les longues vidéos demandent davantage de temps et de mémoire. L’aperçu HTMLVideoElement est vérifié avec des médias synthétiques ; l’identité des images exportées est contrôlée séparément par les tests natifs.

## Brouillons

Points de coupe, gains et sourdines sont conservés sur cet appareil, indépendamment du MP4. Annuler/rétablir agit sur ces réglages ; réinitialiser ne modifie jamais l’original. Une erreur de sauvegarde locale est affichée dans l’éditeur.

## Exports en arrière-plan

L’éditeur démarre un job et affiche son avancement dans la barre latérale. On peut naviguer pendant l’export. Un seul export lourd, synchrone ou asynchrone, est admis à la fois. Le changement du dossier des clips est refusé pendant le travail pour préserver la cohérence de la bibliothèque.

L’annulation est coopérative : les lectures, copies, mixages et écritures vérifient le jeton. Un appel natif déjà lancé finit avant que le jeton soit relu. Une demande après le point de publication peut arriver trop tard ; le statut final indique alors un export terminé. Une annulation confirmée nettoie les sorties partielles et ne publie pas de clip.

## Protocole

Les arguments sont au même niveau que `id` et `cmd` dans les messages du pipe :

```json
{
  "id": 1,
  "cmd": "library.export.start",
  "request_token": "unique-client-attempt-1",
  "clip_id": "20261026_120000",
  "start_ms": 0,
  "end_ms": 15000,
  "mode": "precise",
  "output": "share",
  "tracks": [{ "index": 1, "gain": 100 }, { "index": 2, "gain": 75 }]
}
```

`output` accepte `master` ou `share`. Son absence conserve le comportement historique `master`. `tracks` absent conserve les flux du maître, ou sélectionne son mix initial pour `share`. `tracks: []` supprime volontairement l’audio. Les indices audio commencent à zéro. Ne pas demander simultanément le mix initial et ses sources : cela doublerait le son.

`size_limit_mb` (1 à 4000, optionnel) fixe la taille maximale en mégaoctets (10⁶ octets). Il n’est accepté qu’avec `mode: "precise"` et `output: "share"`. Le clip publié porte la définition réellement écrite et un titre suffixé « (N Mo) ». `library.copy {clip_id}` place le MP4 du clip dans le presse-papiers.

`mode` accepte `fast` ou `precise` ; son absence conserve `fast` pour les clients historiques. `library.frames {clip_id}` répond `{timestamps_100ns, end_100ns}` : PTS vidéo triés et uniques, en unités de 100 ns, et fin exclusive réelle. `library.keyframes` reste disponible pour les clients existants.

`request_token` est optionnel pour les clients existants, non vide et limité à 128 octets lorsqu’il est fourni. L’interface en crée un par demande. Un réessai avec la même clé et les mêmes paramètres retrouve le job ; des paramètres différents sont refusés. La corrélation et l’idempotence couvrent les 16 jobs conservés en mémoire dans ce daemon ; un redémarrage ou l’éviction d’un résultat termine cette fenêtre.

Réponse de démarrage : snapshot `{job_id,request_token?,state,percent,stage,clip?,error?}`. États : `queued`, `running`, `cancelling`, `completed`, `failed`, `cancelled`. Le pourcentage mesure le travail traité, pas un temps restant.

```json
{"id":2,"cmd":"library.export.status","job_id":"returned-job-id"}
{"id":3,"cmd":"library.export.status","request_token":"unique-client-attempt-1"}
{"id":4,"cmd":"library.export.cancel","job_id":"returned-job-id"}
```

Les sélecteurs `job_id` et `request_token` sont exclusifs. Sans sélecteur, le statut retrouve le job actif ou le dernier résultat. Par jeton inconnu, il répond `null`, sans attribuer le résultat d’un ancien export à une nouvelle demande refusée.

La commande historique `library.export` reste synchrone pour compatibilité. Les nouveaux clients doivent utiliser les jobs pour ne pas bloquer leur canal de contrôle. Les erreurs audio échouent explicitement ; aucun repli silencieux vers une vidéo muette.

## Vérification

Tests du protocole, du scheduler, de la progression, de l’annulation et de la reprise frontend. Un MP4 synthétique H.264 avec trois pistes AAC vérifie réellement : copie maître multipiste, sortie partage à un mix, gains appliqués et source identique octet par octet. Un test natif du worker vérifie la publication et l’absence de doublon lors d’un réessai corrélé. Aucun écran personnel n’est capturé.

Les tests précis décodent les sorties à 10 et 60 fps pour comparer la première et la dernière image aux images source, vérifier le nombre d’images et leurs timestamps. Le cas 17 ms à 60 fps est couvert. Le test PCM compare exactement les échantillons rognés aux échantillons de la source ; un autre décode l’audio exporté en entier et borne le padding AAC. Master, Partage, silence volontaire et annulation sont couverts.

Plafond de taille : tests unitaires du plan (conservation, paliers, portrait, ultra-large, refus), du rééchantillonnage NV12 (aplats, plan UV après lignes de remplissage, dégradé), de l’appariement des timestamps à 1 ms ; test natif d’un export plafonné sous la limite et d’un export redimensionné à 80×48 qui conserve chaque image. Test opt-in `real_clip_fits_the_discord_free_cap` (`CLIPPY_REAL_CLIP`, `CLIPPY_REAL_OUT`) : le 31 octobre 2026, 30 s d’un vrai clip H.264 1080p à fréquence variable ont donné 12,8 Mo en 1280×720, 1 721 images, en 14,5 s (build release, encodeur logiciel). Image décodée comparée visuellement à la source. Les fichiers AV1 ne sont pas décodables par cette voie et l’export l’indique.

Les tests matériels ou réseau ignorés restent distincts de ces preuves. La vérification navigateur utilise un faux transport IPC et des médias synthétiques ; elle valide les commandes et l’interface, pas la capture GPU ni le protocole asset dans WebView2.

## Format vertical 9:16

`vertical_center` (0..=1000, 500 = centre) sur `library.export(.start)` découpe une colonne 9:16 de la hauteur
source (`export::vertical_crop` : bords pairs, bornée dans l'image), puis l'encode à sa taille native
(606×1080 pour du 1080p, sans agrandissement). Il demande la coupe précise et se combine avec `size_limit_mb`
(le plan de débit porte sur le rectangle recadré). Dans l'éditeur : sélecteur « Format de l'image »,
curseur « Cadrage », et cadre déplaçable à la souris sur la vidéo.
