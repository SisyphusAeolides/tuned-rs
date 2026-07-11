pkgname=tuned-rs
pkgver=1.0.0
pkgrel=1
pkgdesc="System tuning daemon for Sisyphus Arch"
arch=('x86_64')
url="https://github.com/SisyphusCode/tuned-rs"
license=('GPL3')
depends=('glibc')
makedepends=('cargo')
source=()

build() {
  cd "$srcdir/.."
  cargo build --release --locked
}

package() {
  cd "$srcdir/.."
  mkdir -p "$pkgdir/usr/bin" "$pkgdir/usr/lib"
  for f in target/release/*; do
    if [ -f "$f" ] && [ -x "$f" ]; then
      if [[ "$f" == *.so ]]; then
        install -Dm755 "$f" "$pkgdir/usr/lib/$(basename "$f")"
      elif [[ "$f" != *.d && "$f" != *.rlib ]]; then
        install -Dm755 "$f" "$pkgdir/usr/bin/$(basename "$f")"
      fi
    elif [ -f "$f" ]; then
      if [[ "$f" == *.so ]]; then
        install -Dm755 "$f" "$pkgdir/usr/lib/$(basename "$f")"
      elif [[ "$f" == *.a ]]; then
        install -Dm644 "$f" "$pkgdir/usr/lib/$(basename "$f")"
      fi
    fi
  done
}
