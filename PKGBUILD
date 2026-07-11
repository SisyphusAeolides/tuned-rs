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
  cargo build --release --locked
}

package() {
  install -Dm755 target/release/$pkgname "$pkgdir/usr/bin/$pkgname"
}
