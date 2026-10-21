# Cross field: QuadLoom `20261023-m3b6` vs oracle `20261021-oracle-baseline`

70 cases; compared on 60 with an oracle field. All singularities are measured with QuadLoom's index routine.

| Median | Oracle | QuadLoom, same mesh | QuadLoom, own pipeline |
|---|---|---|---|
| Singularities | 100 | 93 | 104 |
| Singularities per 1k faces | 5.70 | 5.89 | 5.54 |
| Σ\|index\| (quarters) | 102 | 95 | 105 |
| Field time (ms) | — | 53 | 66 |

| Case | Oracle sing. | Same-mesh sing. | Ours sing. | Ours faces | Curvature-aligned |
|---|---|---|---|---|---|
| common3d/alligator | — | — | 30 | 152809 | False |
| common3d/armadillo | 406 | 477 | 334 | 37270 | True |
| common3d/beetle | 117 | 104 | 96 | 15255 | False |
| common3d/cheburashka | 101 | 87 | 94 | 14912 | True |
| common3d/cow | 234 | 230 | 205 | 19252 | False |
| common3d/fandisk | 32 | 32 | 32 | 10876 | False |
| common3d/homer | 134 | 121 | 115 | 16940 | True |
| common3d/igea | 134 | 141 | 126 | 21422 | True |
| common3d/max-planck | 36 | 45 | 55 | 17418 | True |
| common3d/rocker-arm | 111 | 96 | 63 | 20986 | True |
| common3d/stanford-bunny | 184 | 152 | 125 | 22043 | True |
| common3d/suzanne | 145 | 147 | 166 | 14662 | False |
| common3d/teapot | 36 | 38 | 14 | 14965 | True |
| common3d/woody | — | — | 101 | 153551 | True |
| crane/blub | 35 | 48 | 41 | 12366 | True |
| crane/bob | 20 | 20 | 20 | 11438 | True |
| crane/crumpleddevelopable | 133 | 134 | 145 | 156141 | False |
| crane/ogre | 120 | 116 | 158 | 21898 | True |
| crane/oloid | 73 | 76 | 10 | 9616 | True |
| crane/pig | 140 | 126 | 111 | 20068 | True |
| crane/spot | 64 | 68 | 68 | 15372 | True |
| khronos/ABeautifulGame | — | — | 1608 | 142221 | False |
| khronos/AntiqueCamera | 364 | 378 | 1666 | 160772 | False |
| khronos/Avocado | 10 | 14 | 14 | 11323 | True |
| khronos/BarramundiFish | 87 | 85 | 87 | 12564 | False |
| khronos/BoomBox | 61 | 59 | 82 | 14758 | False |
| khronos/BrainStem | — | — | 3556 | 150969 | False |
| khronos/CesiumMan | 66 | 79 | 58 | 12026 | False |
| khronos/CesiumMilkTruck | 38 | 49 | 304 | 19962 | False |
| khronos/ChronographWatch | — | — | 2491 | 137070 | False |
| khronos/CommercialRefrigerator | 340 | 377 | 1169 | 114855 | False |
| khronos/Corset | 379 | 356 | 471 | 27991 | False |
| khronos/DamagedHelmet | 135 | 138 | 503 | 42009 | False |
| khronos/DragonAttenuation | — | — | 2041 | 36847 | False |
| khronos/Duck | 77 | 67 | 58 | 14364 | True |
| khronos/Fox | 98 | 99 | 90 | 12304 | False |
| khronos/GlamVelvetSofa | 23 | 22 | 47 | 17778 | False |
| khronos/Lantern | 51 | 44 | 232 | 22879 | False |
| khronos/MaterialsVariantsShoe | 93 | 90 | 76 | 36191 | False |
| khronos/SciFiHelmet | — | — | 1180 | 41324 | False |
| khronos/SheenChair | 205 | 151 | 224 | 44327 | False |
| khronos/SheenCloth | 14 | 12 | 2 | 29226 | False |
| khronos/SpecularSilkPouf | 698 | 714 | 340 | 60976 | False |
| khronos/SunglassesKhronos | 223 | 239 | 181 | 161328 | False |
| khronos/ToyCar | — | — | 402 | 50471 | False |
| khronos/TrafficCone | — | — | 375 | 153781 | True |
| khronos/WaterBottle | 24 | 24 | 24 | 16270 | False |
| thingi10k/105762 | 146 | 200 | 164 | 18990 | False |
| thingi10k/124040 | 65 | 66 | 64 | 12604 | False |
| thingi10k/1458674 | 241 | 246 | 247 | 23760 | False |
| thingi10k/1492345 | 58 | 57 | 63 | 21588 | False |
| thingi10k/1505036 | 362 | 365 | 360 | 14910 | False |
| thingi10k/1514900 | 216 | 205 | 415 | 149278 | False |
| thingi10k/1706457 | 71 | 73 | 56 | 16548 | False |
| thingi10k/250397 | 4 | 4 | 7529 | 48244 | True |
| thingi10k/271869 | 542 | 539 | 538 | 35386 | False |
| thingi10k/37883 | 146 | 138 | 154 | 18204 | False |
| thingi10k/40994 | 44 | 36 | 38 | 11282 | True |
| thingi10k/472206 | 24 | 24 | 24 | 15378 | False |
| thingi10k/49382 | 8 | 8 | 10 | 14912 | True |
| thingi10k/62287 | 160 | 159 | 164 | 18182 | False |
| thingi10k/650187 | 0 | 0 | 0 | 125880 | False |
| thingi10k/71712 | 390 | 392 | 384 | 41160 | False |
| thingi10k/75117 | 42 | 44 | 42 | 10920 | False |
| thingi10k/756803 | — | — | 5739 | 31414 | False |
| thingi10k/87355 | 169 | 165 | 120 | 20970 | True |
| thingi10k/906891 | 525 | 622 | 158 | 20386 | False |
| thingi10k/94990 | 46 | 46 | 48 | 11668 | False |
| thingi10k/97730 | 44 | 42 | 42 | 13162 | False |
| thingi10k/99836 | 375 | 357 | 350 | 12260 | False |
