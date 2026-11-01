# ADR — exports en arrière-plan

Statut : accepté pour implémentation. Date : 26 octobre 2026.

## Contexte
L’export synchrone bloque le client pipe qui l’a demandé. Le délai de réponse de 60 secondes peut expirer sans annuler le travail ; un nouvel essai crée alors potentiellement une seconde copie. Allonger uniquement ce délai ne résout ni l’annulation ni la réactivité.

## Décision
Ajouter `library.export.start`, `library.export.status` et `library.export.cancel`, en conservant `library.export` pour les clients existants. Un seul export actif dans le daemon, historique terminal borné à 16. Le démarrage réserve une place sous verrou puis rend immédiatement un identifiant unique ; un thread natif possède un Arc du daemon et crée ses objets COM/Media Foundation localement. Aucun verrou de jobs n’est conservé pendant le traitement média.

Le moteur expose un contrôle de progression et une annulation coopérative. Les boucles d’indexation, décodage, copie, mixage et encodage vérifient le jeton. Une annulation supprime les sorties partielles ; elle ne publie pas de clip. `Finalize` et les appels MF individuels ne peuvent être interrompus au milieu d’un appel : l’annulation est prise en compte au prochain point de contrôle.

L’interface conserve le suivi dans un store indépendant du montage. Navigation et reconnexion ne lancent jamais un second export. Le statut peut retrouver le job actif ou le dernier résultat sans identifiant, et porte le résultat ou l’erreur, même lorsqu’un événement a été perdu.

## Contrat
Snapshot : `{job_id,state,percent,stage,clip?,error?}`.
États : queued, running, cancelling, completed, failed, cancelled.
Les états terminaux libèrent la place. Une annulation demandée reste visible jusqu’à confirmation du moteur. Le pourcentage décrit le travail traité, pas une estimation de temps restant.

## Alternatives
- Timeout plus long : faible coût mais blocage, doublons et absence d’annulation persistants ; rejeté.
- File illimitée de travaux : complexité et concurrence inutiles pour l’outil ; rejetée.
- Thread natif borné + statut : coût limité, pas de runtime async supplémentaire ; retenu.

## Vérification
Tests du scheduler (capacité, spawn échoué, annulation, états terminaux bornés), du moteur (jeton et nettoyage réel), du protocole, du suivi frontend (double démarrage, réponses tardives, reconnexion), puis scénario navigateur. Le MP4 synthétique H264/AAC permet de vérifier les sorties et l’intégrité du maître sans capturer l’écran.
