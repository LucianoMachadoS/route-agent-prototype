# route-agent

> ⚠️ Nota de recuperação: o `README.md` original deste projeto foi
> sobrescrito por acidente e não pôde ser recuperado (sem backup, sem git).
> Este arquivo foi reconstruído a partir do `Cargo.toml`, de
> `docs/experiments.md` e dos templates em `examples/`. Se você tinha
> detalhes adicionais em mente que não estão capturados aqui, me avise para
> eu completar.

Controlador em Rust (protótipo) para um túnel do **WireGuard for Windows
oficial**, com dois objetivos:

1. Automatizar subir/derrubar o túnel (`wireguard.exe /installtunnelservice`
   / `/uninstalltunnelservice`) em vez de clicar na UI do WireGuard.
2. Medir, de forma repetível, se a rota **via um relay seu** (ex: uma VPS em
   outra região/provedor) é mais rápida/estável até um servidor de jogo do
   que a rota direta da sua operadora — seguindo o protocolo descrito em
   [`docs/experiments.md`](docs/experiments.md).

Esta **não** é uma VPN de saída geral: como definido nos templates em
`examples/`, o `AllowedIPs` do túnel é restrito a `10.66.0.0/24` (a sub-rede
privada cliente↔relay), não `0.0.0.0/0`. O relay só aceita pacotes do IP do
túnel do cliente (`10.66.0.2/32`). Ou seja, por padrão isto mede **latência
cliente→relay**, não a rota completa até o jogo — é exatamente o que a nota
em `docs/experiments.md` avisa.

## Por quê

Serviços pagos de "otimização de rota para jogos" (ExitLag, WTFast, Mudfish
etc.) basicamente fazem isto: mantêm uma rede de relays e escolhem, por
jogo/servidor, qual relay dá a rota mais rápida que a sua operadora não
oferece direto. Este projeto é a versão "faça você mesmo": você aluga um
relay (qualquer VPS barata, Linux, em uma região perto do datacenter do
jogo), sobe um túnel WireGuard até ele com o cliente oficial do Windows, e
usa este CLI para medir e automatizar isso — sem depender de licença de
terceiro nem mexer no binário de ninguém.

## Pré-requisitos

- [WireGuard for Windows](https://www.wireguard.com/install/) instalado
  (o CLI chama `wireguard.exe`, normalmente em
  `C:\Program Files\WireGuard\wireguard.exe`).
- Um relay Linux com WireGuard, configurado a partir de
  `examples/relay-wg0.conf.template`.
- Rust (`cargo`) para compilar.
- PowerShell/terminal **como administrador** para `up`/`down` (instalar
  serviço do Windows exige elevação).

## Configurando o relay e o cliente

1. No relay (sua VPS Linux): copie `examples/relay-wg0.conf.template` para
   `/etc/wireguard/wg0.conf`, gere um par de chaves (`wg genkey | tee
   relay.key | wg pubkey > relay.pub`) e preencha `<RELAY_PRIVATE_KEY>`.
   Depois de ter a chave pública do cliente (próximo passo), preencha
   `<CLIENT_PUBLIC_KEY>`. Suba com `wg-quick up wg0` ou via systemd.
2. No seu Windows: copie `examples/client.conf.template` para um arquivo
   `.conf` fora do controle de versão (ex: `%USERPROFILE%\wg-client.conf`),
   gere seu par de chaves e preencha `<CLIENT_PRIVATE_KEY>`,
   `<RELAY_PUBLIC_KEY>` e `<RELAY_PUBLIC_IPV4_OR_DNS>`.
   **Nunca** commite um `.conf` com chaves reais — os templates existem
   justamente para isso.

## Uso do CLI

```powershell
cargo build --release

# Sobe o túnel (instala e inicia como serviço do Windows) - requer admin
route-agent up --config "C:\Users\voce\wg-client.conf"

# Verifica se o serviço do túnel está rodando
route-agent status --name wg-client

# Compara latência direta vs. via túnel (grava uma linha em docs/runs.csv)
route-agent bench --direct-host <ip-ou-host-do-servidor-do-jogo> --tunnel-host 10.66.0.1 --count 30 --note "relay=fra1,isp=vivo-fibra"

# Derruba o túnel (desinstala o serviço) - requer admin
route-agent down --name wg-client
```

`bench` implementa o worksheet de `docs/experiments.md`: mede mediana, p95,
p99, jitter e taxa de perda de pacotes para o host direto e para o host via
túnel, imprime a comparação e acrescenta uma linha em `docs/runs.csv` para
você comparar execuções ao longo do tempo variando uma coisa por vez (relay,
operadora, horário etc.).

## Estrutura

```
Cargo.toml                       # clap (CLI), thiserror (erros), tempfile (testes)
src/main.rs                      # subcomandos: up / down / status / bench
docs/experiments.md              # protocolo de medição (o que registrar em cada run)
docs/runs.csv                    # histórico de execuções do `bench` (gerado)
examples/client.conf.template    # config do cliente Windows (preencher chaves)
examples/relay-wg0.conf.template # config do relay Linux (preencher chaves)
```

## Status

Protótipo. Implementado: `up`, `down`, `status`, `bench`. Em aberto para
próximas iterações:

- Roteamento seletivo de apenas o tráfego do jogo pelo túnel (hoje é tudo ou
  nada dentro do `AllowedIPs` do túnel) — precisa de NAT/forwarding no relay
  para o IP de destino do jogo, não só aceitar o `/32` do cliente.
- Seleção automática de relay (testar vários e escolher o melhor por jogo).
- Persistir config de qual jogo usa qual relay.
