# Baseline Trix v1.5.0 — RTX 4060 Ti (run 1, bureau quasi statique)

Config : 1920x1080 @ 60 fps cible, 8000 kbps VBR, replay 15 s, capture auto, NVIDIA H.264 MFT.
Build : release, cargo 1.93.1. Tests : trix-core 145 ok / 6 ignorés, trix-daemon et proto ok.

| Mesure | Valeur |
|---|---|
| Frames capturées (≈50 s) | 837 (écran statique : WGC ne livre que sur changement) |
| Dropped / paced | 0 / 0 |
| Latence on_frame | p50 ≤ 500 µs, p99 ≤ 1,0 ms, max 3,7 ms |
| Working set | 89,6 Mo |
| VRAM dédiée / partagée imputée | 35 / 19 Mo |
| Ring CPU | 4,7 Mo |
| Clip (15,7 s) | 266 paquets, 1,8 Mo, mux 150 ms |

Limite : ce run n'a pas de charge. Pas de mesure à 60 fps réels, ni de frames perdues sous clip.
À refaire pendant un jeu (ou animation plein écran). Log : baseline-run1.log
