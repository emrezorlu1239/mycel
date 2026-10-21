# Mycel

**Arkadaş grupları için, aradaki kimsenin kapatamayacağı eşten eşe (P2P) sohbet uygulaması.**

[English](README.md)

Mycel, şirket sunucusu olmayan, Discord tarzı bir sohbet uygulamasıdır. Grubu
bir arkadaşın bilgisayarı barındırır, diğerleri doğrudan ona bağlanır ve tüm
trafik şifrelidir. Kapatılacak bir hesap, engellenecek merkezi bir sunucu yoktur;
veriler yalnızca sizin cihazlarınızda durur.

İsim, ormandaki ağaçları yer altından birbirine bağlayan mantar ağı *miselyum*dan
gelir: merkezi yoktur, bir kolu kesilse de ağ yaşamaya devam eder.

> **Durum: Faz 0 (kavram kanıtı).** Ağ çekirdeği çalışıyor ve test edildi;
> henüz grafik arayüz yok. Yol haritası: [docs/roadmap.md](docs/roadmap.md).

## Temel fikirler

- **Tasarımı gereği engellenemez.** Kapatılabilecek merkezi sunucu, alan adı ya
  da hesap sistemi yok. Cihazlar birbirini davet koduyla bulur ve doğrudan
  bağlanır; ancak modemler başka yol bırakmazsa aracı (relay) kullanılır.
- **Grubun kuralları sizin.** Host aynı zamanda admindir: kimin gireceğine o
  karar verir, sunucuyu başka bir üyeye devredebilir.
- **Veriler evde kalır.** Mesajlar ve dosyalar host'un ve üyelerin kendi
  cihazlarında, istedikleri an silebilecekleri bir klasörde durur. Canlı ses
  hiçbir zaman kaydedilmez.
- **Herkes için.** Hedef, teknik bilgisi olmayan arkadaşlarınızın modem ayarına
  dokunmadan kurup kullanabileceği bir uygulama.

## Hızlı deneme (Faz 0 komut satırı)

[Rust](https://rustup.rs) 1.88 veya üstü gerekir. Windows'ta ayrıca
*Visual Studio Build Tools* içinden "Desktop development with C++" kurulmalıdır.

```bash
cargo build --release
./target/release/mycel host --server "Cuma Akşamı" --nick emre     # host
./target/release/mycel join mycel1... --nick ali                    # arkadaşın bilgisayarı
```

Aynı Wi-Fi ağında doğrudan çalışır. Farklı evler arasında deneme adımları için:
[docs/testing-with-friends.md](docs/testing-with-friends.md).

## Lisans

[MIT](LICENSE)
