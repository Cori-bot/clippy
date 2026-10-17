# Matrice 1 — résolution x fps (RTX 4060 Ti, moniteur 1920x1080 @144 Hz, vidéo 4K60 en boucle)

Données : matrix-1.csv (1 run par cas, 30 s, clip auto à 20 s, ring 15 s, bitrate auto). Harnais : scripts/bench-matrix.py.

## Ce qui marche
- 16/16 combinaisons produisent un MP4 de la résolution et du fps demandés (480p = 852x480 ... 1440p = 2560x1440).
- on_frame p99 <= 2 ms partout ; encode p99 <= 2 ms ; verrou de snapshot <= 2,7 ms ; mux 160-270 ms.
- Encodeur NVENC : 1,0 % (480p30) -> 29,8 % (1440p120). GPU 3D ~12-16 % (dominé par la vidéo lue).
- RAM (working set) : 92 Mo (480p) -> 141 Mo (1440p120) ; ring CPU 8 -> 52 Mo (proportionnel au bitrate auto).

## Défauts observés
1. **fps 90 -> ~74-77 fps réels, quelle que soit la résolution** (aussi 73-107 attendus idem). Cause : MinUpdateInterval WGC = 0,75/fps = 8,3 ms > période vsync 144 Hz (6,9 ms) ->
   WGC ne livre qu'un vsync sur deux (72 Hz). Corrigeable : ne pas plafonner quand fps > refresh/2.
2. Frames perdues non nulles à bas niveau (720p60 : 8, 720p90 : 20, 480p120 : 4) : à localiser (démarrage ? backpressure ? clip ?).
3. CPU : ~3,3 % de base + ~0,09 % par fps (soit ~0,9 ms de CPU par frame) -> piste : vues VideoProcessor recréées à chaque frame ; base fixe -> audio/threads.
