# GamePrep — otimizador de Windows para jogos

Fica minimizado vigiando os jogos que você escolheu. Quando um deles
abre, **fecha sozinho os apps de segundo plano** que você marcou (Discord,
Spotify, navegador, etc.) para sobrar CPU, RAM e VRAM para o jogo. Quando
o jogo fecha, pode reabri-los.

Não tem nada a ver com o túnel/relay do `route-agent`: é um programa à
parte, com instalador próprio.

## O que ele faz

- **Modo jogo automático:** detecta o jogo em até 2 s e fecha os apps da
  lista. Primeiro pede para fecharem normalmente, como clicar no X
  (navegadores guardam as abas). Só encerra à força o que não fechar em
  5 s, como apps que "fecham para a bandeja" (Discord, Spotify).
- **Reabrir depois** (opcional): ao fechar o jogo, reabre o que foi fechado.
- **Prioridade do jogo** (opcional, desligado por padrão): "acima do
  normal". Raramente ajuda e pode piorar áudio e voz.
- **Gravação do Xbox Game Bar:** mostra se está ligada (custa FPS) e
  desliga ou religa com um clique.
- **Diagnóstico:** o mesmo `route-agent diagnose` — rede, driver, roteador,
  RAM, GPU/VRAM, energia, inicialização — só lê, não muda nada.
- **Iniciar com o Windows** minimizado (liga e desliga pelo app; aparece
  igual no Gerenciador de Tarefas > Aplicativos de inicialização).

## Segurança

- **Nunca roda como administrador.** Tudo o que ele toca é do seu usuário:
  os seus processos e o registro `HKCU`. O instalador pede UAC uma vez,
  só para copiar o programa para `Program Files`. Testado: o app aberto no
  fim da instalação não é elevado.
- **Lista de protegidos** (`src/protect.rs`): Windows, antivírus, Steam,
  Riot Client/Vanguard, Epic/EA/Battle.net/FACEIT, drivers de vídeo e
  áudio, terminais e o próprio GamePrep nunca aparecem para fechar, mesmo
  se alguém editar a configuração à mão.
- Configuração em `%APPDATA%\GamePrep\config.toml`; log com rotação em
  `%LOCALAPPDATA%\GamePrep\gameprep.log`.

## Instalar

Rode `dist\GamePrep-<versão>-x64.msi`. Ele instala em
`C:\Program Files\GamePrep`, cria o atalho no menu Iniciar, liga a
inicialização com o Windows e abre o app. Para desinstalar, use
Configurações > Aplicativos. Se o GamePrep estiver aberto, o Windows
pergunta se pode fechá-lo: escolha fechar automaticamente.

**O instalador não é assinado digitalmente.** Num arquivo baixado da
internet, o Windows SmartScreen mostra "O Windows protegeu o computador"
(clique em *Mais informações* > *Executar assim mesmo*) e antivírus podem
desconfiar. Só uma assinatura de código paga resolve isso de verdade.

## Compilar

```powershell
cargo test -p gameprep
.\gameprep\installer\build-msi.ps1     # gera dist\gameprep.exe e dist\GamePrep-<versão>-x64.msi
```

O `.msi` precisa do [WiX Toolset v7](https://github.com/wixtoolset/wix/releases)
(`wix-cli-x64.msi`, assinado pela FireGiant). O script aceita a licença
OSMF do WiX (`-acceptEula wix7`), cuja taxa só vale para organizações com
mais de US$ 10 mil/ano de receita.
