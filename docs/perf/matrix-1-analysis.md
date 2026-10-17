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

## Suite (matrice 2 et mesures ciblées)

### Correctif WGC (fps 73..107 sur 144 Hz) — mesuré
`MinUpdateInterval = 0,75/fps` dépassait la période vsync (6,9 ms) : WGC livrait 1 vsync sur 2 (72 Hz).
Pire : `MinimumUpdateIntervalSettings::Default` n'est PAS « sans plafond » (~48 fps mesurés, 1 vsync sur 3).
Correctif : si le plafond quantifierait sous la cible, intervalle explicite d'un demi-vsync (`update_interval_cap_is_safe`, `mod.rs`).

| cible | avant (fps réels) | après |
|---|---|---|
| 75 | ~74-77 (sauf 72 en dessous) | 76,0 / 76,2 |
| 90 | 74-77 | 91,0 / 90,6 |
| 100 | ~74-77 | 100,7 / 96,7 |
(matrix-2-wgc-cap.csv, 720p et 1080p ; mp4 : 74,9 / 89,7 / 98,6 fps)

### Cache des vues VideoProcessor — essayé, abandonné
A/B 3 rounds old/new à 1080p60 et 120 : CPU médian 7,5 % -> 7,9 % (60 fps), 10,1 % -> 11,9 % (120 fps), bruit de mesure
(le contenu à l'écran varie : fps_eff 81 vs 108 sur 120). Aucun gain démontré -> changement annulé.

### Où va le CPU (scripts/thread-cpu.ps1, 1080p60, total 7,3 % d'un coeur ~ 0,45 % du Ryzen 16 threads)
Thread de callback de capture ~3,8 % (~0,6 ms/frame : soumission VideoProcessorBlt + MFT dans le pilote), un thread MF ~2 %, audio 0,13 %.
Pas de base fixe notable : le CPU est au plancher de ce que permet la soumission via Media Foundation.

### Frames perdues (backpressure)
Rares (0 à 20 sur 2000+, <1 %), non reproduites en 4 runs ciblés avec journalisation ; diagnostic gardé (20 premiers drops, niveau debug).

### Gates PowerShell
protocol-smoke : PASS (étape 10 corrigée : `replay` ne peut pas démarrer à côté du daemon qui garde Alt+F10 enregistré ; on utilise `record`).
arm-cycle-leak : PASS (0,37 Mo/cycle, handles 3,6/cycle). daemon-smoke : pas de FAIL.
