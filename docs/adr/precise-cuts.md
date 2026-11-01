# Coupe précise sur les timestamps vidéo

Statut : implémenté. Date : 27 octobre 2026.

Le passthrough H.264 dépend des images clés et ne permet pas de respecter une entrée choisie image par image. L’éditeur utilise désormais un index des PTS réels plutôt qu’une cadence nominale. Les bornes sont rapprochées du PTS le plus proche, égalité vers le passé ; la sortie est exclusive.

La voie précise décode NV12 puis réencode H.264 avec Media Foundation. Le PCM est rogné sur une grille globale d’échantillons, ce qui évite les erreurs dues à l’arrondi des timestamps des paquets AAC. Les pistes du maître restent indépendantes en l’absence d’une sélection de remix. Les jobs, jetons de requête, progression et annulation restent communs aux deux modes.

Le choix conserve le moteur de capture existant et n’ajoute aucune dépendance média. En contrepartie, il ajoute une génération de compression, décode actuellement depuis le début et prépare l’audio en mémoire. La copie rapide reste un choix explicite pour éviter le réencodage. Une accélération ou un seek optimisé pourra venir après mesure, en gardant les mêmes tests d’identité des images.
