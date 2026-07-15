#!/usr/bin/env bash
# Installation facile du plugin loopwire (Rust) pour OBS Studio
# (utilisateur courant uniquement, aucun accès root nécessaire).
#
# ATTENTION : ce script ne fait QUE compiler + copier le plugin, il
# n'installe aucune dépendance lui-même. Il faut avoir installé au
# préalable : un compilateur Rust (cargo), un compilateur C++, les
# en-têtes de développement d'OBS Studio, les en-têtes de développement
# de Qt6 (QtWidgets/QtGui/QtCore), pkg-config, et libclang (utilisé par
# bindgen pour lire les en-têtes d'OBS). Les paquets exacts diffèrent
# selon la distribution :
#
# Note : OBS Studio lui-même n'est PAS dans ces listes — vous l'avez
# forcément déjà (vous installez un plugin pour lui). Ne le réinstallez
# pas via ces commandes : ça pourrait le mettre à jour vers une version
# différente de celle actuellement installée et casser la compatibilité
# ABI avec le plugin compilé. Si OBS Studio n'est vraiment pas encore
# installé, installez-le d'abord séparément, normalement.
#
#   Arch / CachyOS / Manjaro :
#     sudo pacman -S --needed rust clang qt6-base pkgconf
#     (les en-têtes de dev d'OBS Studio sont fournis par le paquet
#     obs-studio lui-même sur Arch — déjà présent puisqu'il tourne)
#
#   Debian / Ubuntu :
#     sudo apt install cargo rustc clang libclang-dev build-essential \
#         libobs-dev qt6-base-dev pkg-config
#     (si libobs-dev n'existe pas pour votre version, ajouter le PPA
#     ppa:obsproject/obs-studio)
#
#   Fedora :
#     sudo dnf install cargo rust clang clang-devel gcc-c++ \
#         obs-studio-devel qt6-qtbase-devel pkgconf-pkg-config
#
# Voir README.md pour le détail et le dépannage en cas d'échec.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PLUGIN_DIR="$HOME/.config/obs-studio/plugins/loopwire/bin/64bit"

echo "Ce script va compiler puis installer le plugin LoopWire pour OBS Studio."
echo "Assurez-vous d'avoir installé au préalable les dépendances listées en haut"
echo "de ce script (rust, clang, en-têtes obs-studio et qt6-base, pkg-config) —"
echo "les paquets exacts varient selon votre distribution (Arch/Debian/Fedora)."
read -r -p "Appuyez sur Entrée pour continuer, ou Ctrl+C pour annuler... "

echo "=== Compilation (cargo --release) ==="
(cd "$SCRIPT_DIR" && cargo build --release)

echo "=== Installation dans $PLUGIN_DIR ==="
mkdir -p "$PLUGIN_DIR"
install -m 755 "$SCRIPT_DIR/target/release/libloopwire.so" "$PLUGIN_DIR/loopwire.so"

echo "=== Terminé ==="
echo "Redémarrez OBS pour que le plugin soit chargé."
