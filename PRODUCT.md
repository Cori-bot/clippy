# Product
<!-- impeccable:product-schema 1 -->

## Platform
web

## Users
Joueurs Windows souhaitant conserver les dernières secondes de jeu et retrouver leurs moments.

## Product Purpose
Capturer, retrouver, découper et partager les clips. Priorité confirmée le 26 octobre 2026 : interface et facilité d’utilisation.

## Capabilities and Constraints
Application Windows Rust/Tauri avec interface React. Préserver le moteur GPU, les fichiers MP4 locaux, le replay, les captures, le montage et les pistes audio. Aucun compte requis. Fenêtre minimale actuelle : 880 × 560.

## Product Principles
- Rendre l’état du replay et la prochaine action compréhensibles.
- Retrouver un clip rapidement dans une grande bibliothèque.
- Préserver les performances et les fonctions existantes.

## Brand Commitments
Nom Clippy et logo existants. Interface française. Système existant décrit dans crates/clippy-ui/DESIGN.md.

## Evidence on Hand
README.md et implémentation existante. Les chiffres du README nécessitent une nouvelle mesure avant toute nouvelle promesse.

## Direction confirmée le 26 octobre 2026
Interface épurée, professionnelle, basée en priorité sur OpenAI Apps SDK UI. Compléments cohérents avec ce système quand il manque un composant. Deuxième composition de bibliothèque retenue en sombre. Logo à boucles entrelacées validé. Micro-interactions inspirées de transitions.dev, amicro.dev et lucide-animated.com.

## Objectifs média et édition
Un clip principal facile à retrouver et partager ; conserver les sources audio internes quand elles servent à l’édition. Les dérivés de préécoute ne doivent pas encombrer le dossier. Montage non destructif, niveaux entendus identiques aux niveaux exportés, réglages conservés et historique annuler/rétablir. Les fonctionnalités non implémentées ne sont pas des promesses de la version actuelle.

Priorité éditeur confirmée : coupe précise et navigation image par image. Implémentation : index réel des images, boutons et raccourcis, export H.264 précis en arrière-plan avec progression et annulation. Le mode rapide reste disponible. Preuves : médias synthétiques natifs, tests de logique et aperçu navigateur ; validation matérielle GPU/WebView2 encore distincte.

Partage confirmé le 31 octobre 2026 : copie du fichier dans le presse-papiers pour Discord, export plafonné à 20, 50 ou 500 Mo avec débit et définition ajustés. Aucun envoi en ligne ni compte : le partage reste local.

Ajouté le 31 octobre 2026 : export vertical 9:16 (TikTok, Shorts, Reels) avec cadrage réglable, et clips automatiques sur kills, multi-kills et aces dans CS2 (GSI), League of Legends (Live Client Data) et Valorant (lecture du kill feed dans l’image capturée), désactivés par défaut. Aucune lecture de la mémoire des jeux.
