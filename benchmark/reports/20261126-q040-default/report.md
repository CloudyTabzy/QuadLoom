# Benchmark report — `20261126-q040-default` (quadloom)

Generated 2026-11-26 14:36. 70 cases: **70** ok. Pure-quad results: 69/70.

## Summary (medians over completed cases)

| Metric | Value | Compared run | Δ |
|---|---|---|---|
| Mean scaled Jacobian (primary) | 0.9018 | 0.9222 | -0.0204 |
| Singularities / 1k quads (primary) | 10.21 | 22.38 | -12.1722 |
| Min scaled Jacobian (guard) | -0.9558 | -0.9979 | +0.0421 |
| Mean distance, % diag (guard) | 0.0236 | 0.0367 | -0.0131 |
| Hausdorff, % diag | 0.791 | 0.491 | +0.3002 |
| Mean angle deviation, deg (guard) | 16.56 | 13.75 | +2.8049 |
| Tool time, s | 3.59 | 12.95 | -9.3640 |

## Cases

| Case | Status | Time s | Faces in | Quads out | Pure | Sing/1k | SJ mean | SJ min | Dist % | Hausdorff % |
|---|---|---|---|---|---|---|---|---|---|---|
| common3d/alligator | ok | 27.7 | 5981 | 140802 | yes | 0.4 | 0.720 | -1.000 | 0.0000 | 0.406 |
| common3d/armadillo | ok | 28.7 | 99976 | 18656 | yes | 14.4 | 0.915 | -0.932 | 0.0498 | 1.935 |
| common3d/beetle | ok | 1.7 | 2053 | 11338 | yes | 16.0 | 0.909 | -0.991 | 0.0170 | 0.723 |
| common3d/cheburashka | ok | 1.4 | 13334 | 9460 | yes | 9.7 | 0.925 | -0.989 | 0.1500 | 5.759 |
| common3d/cow | ok | 2.9 | 5804 | 12247 | yes | 22.4 | 0.900 | -0.994 | 0.0244 | 0.613 |
| common3d/fandisk | ok | 0.8 | 12946 | 12298 | yes | 39.5 | 0.937 | 0.093 | 0.0083 | 1.159 |
| common3d/homer | ok | 4.5 | 12000 | 10508 | yes | 12.4 | 0.919 | -1.000 | 0.0512 | 2.466 |
| common3d/igea | ok | 12.3 | 268686 | 12905 | yes | 9.0 | 0.922 | -0.816 | 0.0361 | 0.603 |
| common3d/max-planck | ok | 4.0 | 99991 | 10838 | yes | 5.3 | 0.913 | -0.000 | 0.0427 | 0.933 |
| common3d/rocker-arm | ok | 2.4 | 20088 | 12666 | yes | 5.4 | 0.960 | -0.948 | 0.0453 | 1.033 |
| common3d/stanford-bunny | ok | 9.8 | 69451 | 15164 | yes | 13.6 | 0.908 | -0.834 | 0.0335 | 0.661 |
| common3d/suzanne | ok | 1.8 | 500 | 11117 | yes | 18.7 | 0.915 | -0.991 | 0.0683 | 1.481 |
| common3d/teapot | ok | 1.0 | 6320 | 10743 | yes | 1.3 | 0.933 | -0.496 | 0.0253 | 0.942 |
| common3d/woody | ok | 26.3 | 1267 | 136867 | yes | 0.1 | 0.730 | -0.801 | 0.0000 | 0.035 |
| crane/blub | ok | 1.5 | 14208 | 10652 | yes | 5.6 | 0.907 | -0.867 | 0.0223 | 0.434 |
| crane/bob | ok | 0.9 | 10688 | 7064 | yes | 2.3 | 0.945 | 0.493 | 0.0236 | 0.560 |
| crane/crumpleddevelopable | ok | 23.2 | 512 | 120394 | yes | 1.0 | 0.878 | -0.996 | 0.0029 | 0.363 |
| crane/ogre | ok | 3.1 | 39856 | 13795 | yes | 15.2 | 0.924 | -0.909 | 0.0413 | 0.806 |
| crane/oloid | ok | 13.6 | 8192 | 9711 | yes | 0.8 | 0.754 | -0.015 | 0.0082 | 0.140 |
| crane/pig | ok | 3.9 | 11392 | 11905 | yes | 13.5 | 0.886 | -0.997 | 0.0370 | 0.524 |
| crane/spot | ok | 2.0 | 5856 | 10314 | yes | 9.1 | 0.939 | -0.692 | 0.0236 | 0.592 |
| khronos/ABeautifulGame | ok | 21.4 | 1498828 | 125525 | yes | 36.6 | 0.836 | -0.999 | 0.0480 | 1.049 |
| khronos/AntiqueCamera | ok | 32.4 | 20066 | 157306 | yes | 39.3 | 0.736 | -1.000 | 0.0052 | 0.740 |
| khronos/Avocado | ok | 1.0 | 682 | 10270 | yes | 1.4 | 0.894 | -0.572 | 0.0096 | 0.549 |
| khronos/BarramundiFish | ok | 1.0 | 3864 | 11864 | yes | 14.2 | 0.875 | -0.912 | 0.0149 | 0.414 |
| khronos/BoomBox | ok | 0.9 | 6036 | 10622 | yes | 9.8 | 0.813 | -0.888 | 0.0352 | 0.871 |
| khronos/BrainStem | ok | 11.4 | 61666 | 123854 | yes | 48.7 | 0.918 | -1.000 | 0.0163 | 3.474 |
| khronos/CesiumMan | ok | 3.6 | 4672 | 9576 | yes | 8.6 | 0.917 | -0.950 | 0.0115 | 0.269 |
| khronos/CesiumMilkTruck | ok | 2.9 | 3624 | 15903 | yes | 20.6 | 0.902 | -0.971 | 0.0222 | 1.011 |
| khronos/ChronographWatch | ok | 14.5 | 100002 | 0 | no | — | — | — | 0.0092 | 0.709 |
| khronos/CommercialRefrigerator | ok | 6.8 | 208679 | 57127 | yes | 32.1 | 0.913 | -1.000 | 0.2041 | 1.803 |
| khronos/Corset | ok | 1.6 | 18324 | 18708 | yes | 67.3 | 0.833 | -1.000 | 0.0175 | 0.551 |
| khronos/DamagedHelmet | ok | 1.7 | 15452 | 20371 | yes | 68.6 | 0.799 | -0.997 | 0.0451 | 2.572 |
| khronos/DragonAttenuation | ok | 19.1 | 134995 | 33224 | yes | 181.3 | 0.718 | -1.000 | 0.0368 | 1.495 |
| khronos/Duck | ok | 1.7 | 4212 | 9522 | yes | 6.4 | 0.909 | -0.826 | 0.0252 | 0.364 |
| khronos/Fox | ok | 1.6 | 576 | 10625 | yes | 9.1 | 0.874 | -0.984 | 0.0110 | 0.505 |
| khronos/GlamVelvetSofa | ok | 7.7 | 4196 | 11472 | yes | 23.0 | 0.767 | -0.999 | 0.0331 | 1.518 |
| khronos/Lantern | ok | 2.8 | 5394 | 21107 | yes | 16.8 | 0.887 | -0.959 | 0.0101 | 0.830 |
| khronos/MaterialsVariantsShoe | ok | 1.8 | 22700 | 22375 | yes | 4.3 | 0.804 | -0.989 | 0.0333 | 2.655 |
| khronos/SciFiHelmet | ok | 5.4 | 23358 | 25140 | yes | 131.3 | 0.788 | -0.994 | 0.0534 | 1.103 |
| khronos/SheenChair | ok | 3.4 | 39936 | 45137 | yes | 284.9 | 0.742 | -0.976 | 0.0391 | 4.277 |
| khronos/SheenCloth | ok | 3.1 | 115200 | 14216 | yes | 0.1 | 0.979 | -0.112 | 0.0416 | 0.777 |
| khronos/SpecularSilkPouf | ok | 10.1 | 81280 | 47513 | yes | 109.5 | 0.574 | -1.000 | 0.0403 | 5.042 |
| khronos/SunglassesKhronos | ok | 27.5 | 13396 | 154019 | yes | 2.1 | 0.770 | -1.000 | 0.0034 | 5.535 |
| khronos/ToyCar | ok | 13.2 | 108936 | 36376 | yes | 51.4 | 0.779 | -0.998 | 0.0428 | 1.314 |
| khronos/TrafficCone | ok | 23.2 | 17224 | 144334 | yes | 0.3 | 0.981 | -0.995 | 0.0038 | 0.401 |
| khronos/WaterBottle | ok | 0.6 | 4510 | 9046 | yes | 3.5 | 0.903 | -0.931 | 0.0331 | 0.817 |
| thingi10k/105762 | ok | 12.1 | 30572 | 21937 | yes | 8.0 | 0.862 | -0.997 | 0.0450 | 1.178 |
| thingi10k/124040 | ok | 5.1 | 7384 | 11294 | yes | 5.8 | 0.919 | -0.383 | 0.0110 | 0.172 |
| thingi10k/1458674 | ok | 17.4 | 4438 | 15079 | yes | 16.7 | 0.939 | -0.987 | 0.0202 | 0.650 |
| thingi10k/1492345 | ok | 2.8 | 10424 | 15540 | yes | 4.3 | 0.977 | -0.997 | 0.0224 | 0.268 |
| thingi10k/1505036 | ok | 14.3 | 4410 | 14379 | yes | 27.1 | 0.944 | -0.782 | 0.0123 | 0.722 |
| thingi10k/1514900 | ok | 50.7 | 7066 | 127615 | yes | 3.7 | 0.792 | -0.994 | 0.0041 | 0.551 |
| thingi10k/1706457 | ok | 4.9 | 17748 | 11092 | yes | 6.0 | 0.867 | -0.796 | 0.0361 | 0.594 |
| thingi10k/250397 | ok | 3.5 | 3424 | 43279 | yes | 0.2 | 0.879 | -0.140 | 0.0074 | 0.741 |
| thingi10k/271869 | ok | 13.0 | 6648 | 38741 | yes | 20.2 | 0.828 | -0.992 | 0.0076 | 0.887 |
| thingi10k/37883 | ok | 2.2 | 3302 | 11313 | yes | 15.0 | 0.896 | -0.245 | 0.0188 | 0.683 |
| thingi10k/40994 | ok | 1.5 | 34984 | 8018 | yes | 5.2 | 0.900 | -0.601 | 0.0390 | 1.420 |
| thingi10k/472206 | ok | 3.0 | 32912 | 21598 | yes | 40.7 | 0.965 | -0.192 | 0.0404 | 3.037 |
| thingi10k/49382 | ok | 1.0 | 9190 | 9277 | yes | 0.9 | 0.868 | 0.166 | 0.0317 | 0.279 |
| thingi10k/62287 | ok | 2.6 | 3368 | 12819 | yes | 13.3 | 0.952 | -0.956 | 0.0177 | 1.602 |
| thingi10k/650187 | ok | 22.2 | 25400 | 129000 | yes | 0.0 | 0.975 | -0.114 | 0.0025 | 0.320 |
| thingi10k/71712 | ok | 11.2 | 4488 | 33451 | yes | 11.7 | 0.916 | -0.999 | 0.0022 | 0.366 |
| thingi10k/75117 | ok | 1.3 | 9094 | 9331 | yes | 4.9 | 0.930 | 0.094 | 0.0148 | 1.357 |
| thingi10k/756803 | ok | 20.2 | 13584 | 47902 | yes | 277.8 | 0.715 | -1.000 | 0.0264 | 7.612 |
| thingi10k/87355 | ok | 3.0 | 22752 | 13031 | yes | 10.2 | 0.911 | -0.574 | 0.0401 | 1.492 |
| thingi10k/906891 | ok | 6.6 | 41300 | 14497 | yes | 12.8 | 0.846 | -0.912 | 0.1342 | 3.000 |
| thingi10k/94990 | ok | 2.3 | 27200 | 29488 | yes | 2.0 | 0.825 | -0.757 | 0.0188 | 0.692 |
| thingi10k/97730 | ok | 2.6 | 3110 | 9462 | yes | 3.6 | 0.961 | -0.088 | 0.0230 | 0.928 |
| thingi10k/99836 | ok | 10.6 | 3136 | 12438 | yes | 27.6 | 0.931 | -0.962 | 0.0140 | 0.506 |

## Failures

None.

## Status changes vs compared run

- common3d/alligator: error → ok
- common3d/woody: error → ok
- khronos/ABeautifulGame: timeout → ok
- khronos/BrainStem: timeout → ok
- khronos/ChronographWatch: timeout → ok
- khronos/DragonAttenuation: timeout → ok
- khronos/SciFiHelmet: error → ok
- khronos/ToyCar: error → ok
- khronos/TrafficCone: timeout → ok
- thingi10k/756803: timeout → ok
