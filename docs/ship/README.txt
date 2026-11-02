Clippy 1.6.1
============

Un enregistreur de clips pour Windows, pour les PC qui ne peuvent pas se
permettre de perdre des images. Clippy garde en mémoire les dernières secondes
de votre écran et les écrit dans un fichier quand vous appuyez sur une touche.
La capture et l'encodage se font sur la carte graphique : le coût pour votre
jeu est proche de zéro, et l'application reste discrète pendant que vous jouez.


NOUVEAUTÉS
----------

  1.6.1

  * Qualité vidéo nettement meilleure : la qualité suit désormais la
    définition et la fréquence d'images (Élevée = environ 52 Mb/s en 1080p à
    120 i/s, contre 14 Mb/s avant), avec le profil H.264 High.

  * Valorant : les kills ne sont plus comptés plusieurs fois, et les clips
    automatiques vont dans le dossier du jeu.

  * Les jeux installés sont détectés tout seuls (Steam, Epic, Riot, GOG,
    Ubisoft, EA, Battle.net, Xbox), une application en plein écran n'est plus
    prise pour un jeu, et la capture d'écran Windows non plus. Un profil de jeu
    peut être supprimé.

  * Éditeur : un clic suffit pour ouvrir un clip, formes d'onde visibles,
    une seule barre en bas, et le dernier export ne réapparaît plus à chaque
    lancement.

  * Volume des sons de notification, et bandeau en haut à gauche ou à droite.

  1.6.0

  * Un éditeur de clip complet : coupe précise à l'image près, navigation
    image par image, pistes audio séparées (son du PC, micro, jeu) avec leur
    volume, annuler / rétablir, et exports en arrière-plan avec progression.

  * Partager en un geste : « Copier le fichier » (puis Ctrl+V dans Discord),
    et des exports plafonnés à 20, 50 ou 500 Mo pour passer sous les limites
    de Discord.

  * Format vertical 9:16 pour TikTok, Shorts et Reels, avec un cadre à
    déplacer sur la vidéo.

  * Clips automatiques sur vos kills, multi-kills et aces dans Counter-Strike
    2, League of Legends et Valorant. Paramètres -> Clips. Désactivé par
    défaut ; pour CS2, relancez le jeu après l'activation.

  * Un bandeau « Clip enregistré » en haut à droite, par-dessus le jeu, comme
    dans l'application NVIDIA. Il n'apparaît jamais dans vos enregistrements.

  * Des sons de clip et de capture d'écran plus doux et plus discrets.

  * Profils de jeu : un dossier de clips par jeu, et la liste des jeux
    détectés.

  * Journal de diagnostic (Paramètres -> Application) : enregistre en détail
    ce qui se passe et les performances du PC, pour analyser un problème.
    Désactivé par défaut, tout reste sur ce PC.


POUR COMMENCER
--------------

  1. Lancez l'installateur (Clippy_<version>_x64-setup.exe) : il s'installe
     pour votre utilisateur seulement, sans droits administrateur, avec une
     entrée dans le menu Démarrer. Avec le zip, décompressez tout dans un même
     dossier et gardez les fichiers ensemble : l'application cherche
     l'enregistreur juste à côté d'elle.

  2. Ouvrez Clippy depuis le menu Démarrer (ou lancez clippy-ui.exe depuis le
     dossier du zip). Il démarre l'enregistreur en arrière-plan.

  3. Au premier lancement, une courte configuration demande l'écran, le
     raccourci et le dossier des clips. Cliquez sur « Terminer et activer ».
     Ensuite Clippy démarre avec Windows et active le replay tout seul : rien
     à ouvrir ni à presser. L'icône de la zone de notification indique quand
     il enregistre.

  4. Appuyez sur Alt+F10 pendant votre partie pour enregistrer les 15
     dernières secondes.

  5. Clic droit sur l'icône de la zone de notification pour les réglages
     rapides : enregistrer un clip ou une capture d'écran, résolution,
     fréquence d'images, durée du replay, mode de capture, démarrage avec
     Windows, son du clip, et Quitter.

Windows SmartScreen vous avertira probablement la première fois, car ces
fichiers ne sont pas signés. « Informations complémentaires » -> « Exécuter
quand même ».


CONTENU DU DOSSIER
------------------

  clippy-ui.exe      L'application : vos clips, la lecture et les paramètres.
  clippy-daemon.exe  L'enregistreur d'arrière-plan, dans la zone de
                   notification. L'application (et Windows, à l'ouverture de
                   session) le lance : vous n'avez pas à le démarrer vous-même.
  clippy.exe         Une interface en ligne de commande, pour les scripts et
                   le diagnostic.
  README.txt       Ce fichier.


CONFIGURATION REQUISE
---------------------

  * Windows 10 ou 11, 64 bits. Le son par application demande Windows 11.
  * Une carte graphique avec encodeur H.264 matériel : toute puce Intel, AMD
    ou NVIDIA des dix dernières années environ convient.

Aucun runtime à installer. Les bibliothèques Visual C++ sont intégrées.


VALEURS PAR DÉFAUT
------------------

  Durée du replay          15 secondes
  Fréquence d'images       60 fps
  Débit                    8000 kbps, variable
  Raccourci du clip        Alt+F10
  Raccourci de capture     Alt+F8
  Dossier des clips        %USERPROFILE%\Videos\Clippy
  Limite de la bibliothèque  aucune : Clippy ne supprime jamais un clip pour
                           gagner de la place
  Priorité GPU             faible : votre jeu passe en premier
  Mode de capture          jeux et bureau
  Son du PC                100  (0 à 200 : au-delà de 100, le son est amplifié)
  Applications enregistrées  toutes
  Microphone               100  (0 à 200 : au-delà de 100, le son est amplifié)
  Lancer au démarrage de Windows  désactivé
  Activer le replay au lancement  désactivé
  Présence Discord         activée

Tout se règle dans Paramètres.


OÙ SE TROUVENT LES FICHIERS
---------------------------

  Clips        %USERPROFILE%\Videos\Clippy   (modifiable dans Paramètres)
  Paramètres   %APPDATA%\clippy\config.toml


MISES À JOUR
------------

Clippy demande une fois à github.com, à l'ouverture de l'application, s'il
existe une version plus récente. Rien n'est envoyé à votre sujet : ni compte,
ni identifiant, ni information sur vos clips ou votre PC.

Quand il y en a une, une barre apparaît en haut de la fenêtre. Rien n'est
téléchargé ni installé tant que vous ne cliquez pas sur « Mettre à jour ».
Clippy se remplace alors et redémarre, en conservant vos paramètres et vos
clips.

Pour désactiver la vérification :  Paramètres -> Mises à jour -> Rechercher
les mises à jour.

Clippy ne peut pas se mettre à jour s'il est placé dans un dossier protégé par
Windows, comme Program Files. Il l'indique et renvoie vers la page de
téléchargement.

Si vous mettez à jour en décompressant dans un *nouveau* dossier plutôt qu'en
laissant Clippy se mettre à jour, revérifiez « Lancer au démarrage de Windows »
ensuite. Windows se souvient de l'ancien dossier et continuerait à lancer la
copie remplacée. Clippy le détecte et affiche le réglage comme désactivé ; le
réactiver pointe Windows vers ce dossier-ci.


CAPTURES D'ÉCRAN
----------------

Alt+F8 enregistre une image de l'écran, comme Alt+F10 enregistre un clip. Elle
va dans un dossier Screenshots à l'intérieur du dossier des clips, et dans le
presse-papiers en même temps : vous pouvez la coller directement dans Discord
ou une conversation sans aller chercher le fichier.

Le replay doit être actif. L'image vient de la même capture en direct que les
clips : replay arrêté, il n'y a rien à photographier, et la touche le dit au
lieu de ne rien faire.

L'onglet Captures d'écran montre tout ce que vous avez pris. Cliquez sur une
image pour la voir en grand ; copier, afficher dans le dossier et supprimer
sont sur la carte.

Une capture d'écran joue un autre son qu'un clip, pour distinguer la touche
pressée sans regarder. Paramètres -> Clippy propose un interrupteur pour ce
son, et le raccourci de capture s'y change aussi si autre chose sur votre PC
utilise déjà Alt+F8.


SON DU CLIP
-----------

Clippy joue un court son quand il enregistre un clip, pour vous confirmer que
le raccourci a fonctionné sans quitter votre jeu. Il se joue que la fenêtre de
Clippy soit ouverte ou non.

Pour utiliser le vôtre :  Paramètres -> Clippy -> Fichier son -> Choisir…

mp3, wav, m4a et tout ce que Windows sait lire conviennent. Seules les 10
premières secondes sont utilisées. Clippy convertit votre fichier une fois et
garde sa propre copie : déplacer ou supprimer l'original ne coupe pas le son.

« Rétablir » remet le son de Clippy. Pour couper le son entièrement, utilisez
l'interrupteur « Son du clip » juste au-dessus.

Il n'y a pas de curseur de volume. Réglez le volume de clippy-daemon.exe dans
le mélangeur de volume de Windows.


LIMITES CONNUES DE CETTE VERSION
--------------------------------

  * Si Alt+F10 ne fait rien, autre chose l'utilise déjà : la surcouche NVIDIA
    s'en empare sur beaucoup de machines. Paramètres -> Détection des
    raccourcis permet de choisir une autre combinaison, et le bouton Tester
    indique si Clippy a bien reçu votre touche.

  * Le rognage se fait en mode rapide uniquement. Le point d'entrée tombe sur
    la dernière image clé avant l'endroit choisi : environ une seconde de
    grain dans les clips enregistrés par Clippy, parfois plus loin dans un
    .mp4 venu d'ailleurs. Le rognage à l'image près n'existe pas encore.

  * Clippy ne peut enregistrer que si l'enregistreur de la zone de
    notification tourne, et ouvrir l'application le lance. Pour qu'il soit
    prêt dès la connexion sans ouvrir l'application, activez « Lancer au
    démarrage de Windows » : cela démarre l'enregistreur, pas la fenêtre.

  * En mode « Jeux uniquement », un jeu est reconnu quand son programme est en
    plein écran et ne figure pas dans la liste des applications de bureau
    (navigateurs, lecteurs vidéo, etc.), ou quand vous l'avez ajouté dans
    « Jeux reconnus ». Si le jeu change d'écran en cours de partie, la capture
    reste sur l'écran où elle a démarré jusqu'à la prochaine partie.

  * Le son par application ne distingue pas les onglets d'un navigateur : il
    capture le navigateur entier. Pour garder Spotify sans YouTube, choisissez
    Spotify, pas le navigateur.

  * Les messages de Clippy (clip enregistré, export terminé, problème) ne
    s'affichent pas par-dessus une vidéo lue en plein écran. Ils disparaissent
    d'eux-mêmes après quelques secondes : un message émis pendant le plein
    écran est manqué, pas retardé.


LICENCE
-------

Clippy est sous licence MIT ; le texte complet se trouve dans le dépôt des
sources ci-dessous. Les icônes et les couleurs viennent du système de design
Apps SDK UI d’OpenAI (licence MIT), dont la licence figure à la fin de ce
fichier.

Sources, tickets et nouvelles versions :  https://github.com/OWNER/clippy
