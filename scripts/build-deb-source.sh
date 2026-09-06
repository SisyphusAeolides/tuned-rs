#!/bin/sh
set -eu

case "${1:-}" in
    "") sign_option="" ;;
    --unsigned) sign_option="--no-sign" ;;
    *)
        echo "usage: $0 [--unsigned]" >&2
        exit 2
        ;;
esac

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output_dir=$(dirname -- "$project_root")
package_name=$(dpkg-parsechangelog -l"$project_root/debian/changelog" -S Source)
full_version=$(dpkg-parsechangelog -l"$project_root/debian/changelog" -S Version)
file_version=${full_version#*:}
upstream_version=${file_version%%-*}
source_name="$package_name-$upstream_version"
build_root=$(mktemp -d)

cleanup() {
    rm -rf -- "$build_root"
}
trap cleanup EXIT HUP INT TERM

source_dir="$build_root/$source_name"
file_manifest="$build_root/files"
mkdir -p -- "$source_dir"

cd "$project_root"
git ls-files -z > "$file_manifest"
git ls-files --others --exclude-standard -z >> "$file_manifest"
tar --null --files-from="$file_manifest" --create --file="$build_root/source.tar"
tar --extract --file="$build_root/source.tar" --directory="$source_dir"

mkdir -p -- "$source_dir/.cargo"
cd "$source_dir"
cargo vendor --locked vendor > .cargo/config.toml

cd "$project_root"
source_date_epoch=$(git log -1 --format=%ct)
orig_archive="$build_root/${package_name}_${upstream_version}.orig.tar.xz"
tar --sort=name --mtime="@$source_date_epoch" --owner=0 --group=0 \
    --numeric-owner --exclude="$source_name/debian" \
    --create --xz --file="$orig_archive" \
    --directory="$build_root" "$source_name"

cd "$source_dir"
if [ -n "$sign_option" ]; then
    dpkg-buildpackage --no-pre-clean --build=source -sa -d "$sign_option"
else
    dpkg-buildpackage --no-pre-clean --build=source -sa -d
fi

install -m 0644 "$orig_archive" "$output_dir/"
install -m 0644 "$build_root/${package_name}_${file_version}.debian.tar.xz" \
    "$output_dir/"
install -m 0644 "$build_root/${package_name}_${file_version}.dsc" \
    "$output_dir/"
install -m 0644 "$build_root/${package_name}_${file_version}_source.buildinfo" \
    "$output_dir/"
install -m 0644 "$build_root/${package_name}_${file_version}_source.changes" \
    "$output_dir/"

echo "Source package written to $output_dir"
