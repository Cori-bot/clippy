# ADR : bibliothèque consultable sans alourdir la capture

Statut : proposé — 26 octobre 2026.

## Contexte
L’interface expose seulement le filtre favoris. La sélection est un index dans la liste visible ; ajouter des filtres sans conserver l’identité du clip risque d’ouvrir ou supprimer un autre élément. Le moteur Rust/GPU et le protocole IPC existants restent les contraintes de performance.

## Décision proposée
Conserver React/Tauri et le store observable existant. Ajouter recherche locale sur titre et jeu, filtre jeu et tri date/durée/taille dans une fonction pure. Garder la virtualisation et les favoris par snapshot. Lors d’un changement de critères, retrouver la sélection par ID ; sinon sélectionner le premier résultat. Montrer le nombre de résultats chargés, un état sans résultat et une remise à zéro. Ne jamais annoncer une recherche exhaustive pendant un chargement incomplet ou en erreur.

Séparer les critères de consultation des métadonnées persistées : aucune migration, aucun IPC de recherche supplémentaire. Le coût du filtrage est proportionnel au nombre de clips chargés ; le tri est O(n log n). Ne pas relancer ce calcul à chaque statistique du replay : mémorisation sur clips et critères uniquement.

## Options
- Filtrage local : faible complexité, réutilise les métadonnées déjà chargées ; dépend du chargement complet de la bibliothèque.
- Recherche côté daemon : permet une pagination globale ; impose contrat IPC, index et cohérence des snapshots supplémentaires. À réexaminer si la taille réelle des bibliothèques ou les mesures de latence le justifient.

## Vérification
Tests de recherche accents/casse/mots, combinaison jeu/favoris, tri déterministe sans mutation, conservation de sélection et changements de dossier. Contrôle TypeScript, suite frontend et build. Vérification visuelle aux tailles 1180 × 760 et 880 × 560.

## Suite
- [ ] Valider la maquette de bibliothèque.
- [ ] Implémenter critères et commandes de capture visibles.
- [ ] Renforcer chargement et soumissions de la configuration initiale.
- [ ] Revue React/TypeScript indépendante et validation visuelle.

## Évolution média et éditeur — priorité ajoutée

L’utilisateur demande aussi de travailler le backend, la sortie des vidéos et l’éditeur au niveau de NVIDIA/Medal/Outplayed. Diagnostic : le master est déjà un seul MP4 multipiste ; les .tN.m4a adjacents sont un cache de préécoute. Les conserver hors de la vue des clips et distinguer master éditable et sortie de partage constitue la direction.

Critères de progression :
- [ ] Un seul master MP4 conserve les pistes, un défaut audio ne produit jamais silencieusement un export muet.
- [ ] Les fichiers de préécoute sont rangés dans un cache réservé et nettoyés avec le clip.
- [ ] Les métadonnées exportées décrivent les flux effectivement présents.
- [ ] Les niveaux entendus en aperçu correspondent aux niveaux exportés jusqu’à 200 %.
- [ ] Les réglages de montage survivent à la navigation et peuvent être annulés/rétablis sans toucher au master.
- [ ] Export de partage simple (mix), puis jobs progression/annulation et coupe précise : à concevoir et implémenter après la base fiable.

Les fonctions de recadrage, assemblage et export frame exact ne sont pas annoncées comme disponibles avant implémentation et vérification réelle.

## Sortie partage implémentée

Décision acceptée pour le lot courant : `library.export.output` distingue `master` (défaut protocole, compatibilité) et `share` (défaut de l’éditeur). Le partage produit un unique mix audio ; le maître conserve ses pistes. La politique audio reste distincte du mode de précision de coupe. Contrat et limites : docs/EXPORTS.md.
