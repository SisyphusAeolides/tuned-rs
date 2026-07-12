pkgname=tuned-rs
pkgver=1.0.0
pkgrel=2
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
  make DESTDIR="$pkgdir" PREFIX=/usr BINDIR=/usr/bin install
}
