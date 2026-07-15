#!/usr/bin/env bash
# Installation facile du plugin LoopWire (Rust) pour OBS Studio
# (utilisateur courant uniquement — seule l'installation des dépendances
# système ci-dessous nécessite sudo, jamais l'installation du plugin
# lui-même qui reste dans le profil utilisateur).
#
# Ce script détecte votre distribution (/etc/os-release) et propose la
# commande d'installation des dépendances adaptée — rien n'est installé
# sans votre confirmation explicite. Voir README.md pour le détail par
# distribution et le dépannage en cas d'échec.
#
# Note : OBS Studio lui-même n'est volontairement jamais dans les
# commandes proposées — vous l'avez forcément déjà (vous compilez un
# plugin pour lui). Le réinstaller via ces commandes pourrait le mettre à
# jour vers une version différente de celle actuellement installée et
# casser la compatibilité ABI avec le plugin compilé.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PLUGIN_DIR="$HOME/.config/obs-studio/plugins/loopwire/bin/64bit"

# Dépendances nécessaires (hors OBS Studio lui-même, voir note ci-dessus) :
# un compilateur Rust (cargo), un compilateur C++, les en-têtes de
# développement d'OBS Studio et de Qt6 (QtWidgets/QtGui/QtCore),
# pkg-config, et libclang (utilisé par bindgen pour lire les en-têtes
# d'OBS).

detect_and_install_deps() {
    if [ ! -r /etc/os-release ]; then
        echo "Impossible de détecter votre distribution (/etc/os-release introuvable)."
        echo "Consultez le README.md pour la liste des paquets nécessaires."
        return 1
    fi

    local id id_like
    id="$(. /etc/os-release && echo "${ID:-}")"
    id_like="$(. /etc/os-release && echo "${ID_LIKE:-}")"

    local deps_cmd=""
    case " $id $id_like " in
        *" arch "*|*" manjaro "*|*" cachyos "*|*" endeavouros "*|*" garuda "*)
            echo "Distribution détectée : Arch Linux (ou dérivée)."
            echo "(les en-têtes de dev d'OBS Studio sont fournis par le paquet obs-studio"
            echo "lui-même sur Arch — déjà présents puisqu'il tourne chez vous)"
            deps_cmd="sudo pacman -S --needed --noconfirm rust clang qt6-base pkgconf"
            ;;
        *" fedora "*)
            echo "Distribution détectée : Fedora."
            deps_cmd="sudo dnf install -y cargo rust clang clang-devel gcc-c++ obs-studio-devel qt6-qtbase-devel pkgconf-pkg-config"
            ;;
        *" debian "*|*" ubuntu "*)
            echo "Distribution détectée : Debian/Ubuntu (ou dérivée)."
            deps_cmd="sudo apt install -y cargo rustc clang libclang-dev build-essential libobs-dev qt6-base-dev pkg-config"
            ;;
        *)
            echo "Distribution non reconnue automatiquement (ID: ${id:-inconnu})."
            echo "Consultez le README.md pour la liste des paquets nécessaires."
            return 1
            ;;
    esac

    echo
    echo "Commande d'installation proposée :"
    echo "  $deps_cmd"
    echo
    read -r -p "Exécuter cette commande maintenant ? [o/N] " reply
    case "$reply" in
        [oOyY]*) ;;
        *)
            echo "Ignoré — installez les dépendances vous-même avant de continuer si besoin."
            return 0
            ;;
    esac

    if eval "$deps_cmd"; then
        return 0
    fi

    echo
    echo "Échec de l'installation. Si libobs-dev n'est disponible dans aucun dépôt"
    echo "activé sur votre système (rare — vérifié présent directement sur Debian 13"
    echo "et Ubuntu 24.04+ au moment de l'écriture), essayez le dépôt backports"
    echo "(sudo apt install -t \$(lsb_release -cs)-backports libobs-dev) ou, en"
    echo "dernier recours, le PPA officiel ppa:obsproject/obs-studio. Voir README.md."
    return 1
}

echo "Ce script va compiler puis installer le plugin LoopWire pour OBS Studio."
echo
detect_and_install_deps || echo "Poursuite sans installation automatique des dépendances."

echo
read -r -p "Appuyez sur Entrée pour lancer la compilation, ou Ctrl+C pour annuler... "

echo "=== Compilation (cargo --release) ==="
(cd "$SCRIPT_DIR" && cargo build --release)

echo "=== Installation dans $PLUGIN_DIR ==="
mkdir -p "$PLUGIN_DIR"
install -m 755 "$SCRIPT_DIR/target/release/libloopwire.so" "$PLUGIN_DIR/loopwire.so"

echo "=== Terminé ==="
echo "Redémarrez OBS pour que le plugin soit chargé."
