# Benchmark plan — Fase 3 (seleção de rotas)

Formaliza os cenários de comparação do documento de arquitetura original
do projeto (cinco cenários A-E) para este repositório especificamente,
com os comandos e campos que `bench`/`monitor` já produzem. Ver também
[`docs/experiments.md`](experiments.md) (o worksheet original da Fase 0/1)
e [`docs/SECURITY.md`](SECURITY.md).

## Antes de começar

- Defina um jogo e um servidor/região fixos para toda a bateria de
  testes (ex.: o IP do servidor que o jogo usa nessa partida/região).
- Tenha pelo menos dois relays configurados em `relays.toml` (copie de
  [`relays.toml.template`](../relays.toml.template)).
- Rode tudo no mesmo horário/dia da semana quando possível - latência de
  rede tem variação diária real; comparar manhã vs. horário de pico
  contamina a conclusão.

## Cenários

| Cenário | O que medir | Como, neste repo |
|---|---|---|
| A — Conexão normal (baseline) | RTT/jitter/perda direto ao servidor, sem túnel | `route-agent bench --direct-host <servidor> --tunnel-host <servidor> --count 30` (ignore a coluna "tunnel" - ela existe porque `bench` sempre compara duas colunas) |
| B — Agente sem túnel | Custo de CPU/RAM do próprio processo, parado/ocioso | `route-agent monitor --relays relays.toml --direct-host <servidor> --dry-run --rounds 10` e observe o uso de memória/CPU do processo no Gerenciador de Tarefas durante a execução |
| C — Relay A | RTT/jitter/perda via um relay específico | `route-agent connect --config <relayA.conf>` então `route-agent bench --direct-host <servidor> --tunnel-host <tunnel_host do relayA> --count 30` |
| D — Relay B | Mesma coisa, outro relay | Repita C com o outro `.conf` |
| E — Failover automático | Tempo de detecção + recuperação quando o relay ativo falha | `route-agent monitor --relays relays.toml --direct-host <servidor>` rodando de verdade (sem `--dry-run`); derrube o relay ativo (pare o WireGuard nele) e cronometre, pelo `docs/decisions.csv`, quanto tempo até o "hard failover" trocar de relay |

Repita cada cenário pelo menos 3 vezes, alternando a ordem quando possível.

## Registro

Cada rodada de `bench` já grava em `docs/runs.csv`; cada rodada de
`monitor` grava em `docs/decisions.csv` (ambos gitignored - são dados
locais, não código). Para cada bateria de testes, anote também à parte
(fora do git, ou no `--note` do `bench`):

- Data/hora, jogo e servidor/região.
- Cenário (A-E) e qual relay.
- Observações: congestionamento, atualização do Windows em andamento,
  qualquer coisa fora do comum.

## Critérios de aceitação (definir ANTES de rodar, não depois)

A Fase 3 só deve ser considerada "comprovada" se, comparando múltiplas
rodadas:

- [ ] Algum relay mostra melhora reproduzível no p95 do RTT ou no jitter
      frente ao Cenário A (baseline), não só numa rodada isolada.
- [ ] Nenhuma piora material na perda de pacotes nos relays escolhidos.
- [ ] No Cenário E, o `monitor` detecta a falha e troca de relay dentro
      de um tempo aceitável (defina um número, ex.: "menos de
      `hard_failover_losses` rodadas × `--interval` segundos").
- [ ] `docs/decisions.csv` não mostra troca excessiva (flapping) num
      cenário sem falha real - isso indicaria que a histerese/margem em
      `src/routing/selector.rs` (`SelectorConfig::switch_margin`/
      `cooldown`) está mal calibrada para a rede em questão.
- [ ] CPU/RAM do processo `monitor` (Cenário B) ficam baixos e estáveis
      ao longo do tempo, não crescendo rodada após rodada (isso
      indicaria um leak/acúmulo, não normalidade de medição).

Uma única medição nunca é prova de nada - a variação natural de rede é
real. Se os critérios acima não se sustentarem em várias rodadas, ajuste
os pesos em `relays.toml` (`[scoring]`) ou os parâmetros de
`SelectorConfig` antes de assumir que a Fase 3 "funciona".

## Fora do escopo deste benchmark

FPS/frametime real do jogo não é medido automaticamente aqui (precisaria
de uma ferramenta de captura de present-time como PresentMon, fora do
escopo da Fase 3). Se for medir manualmente por fora, anote o resultado
junto do cenário correspondente, mas não trate como um critério de
aceitação formal ainda.

~~`monitor` mede o caminho direto apenas como referência~~ resolvido: o
caminho direto (`--direct-host`) agora é pontuado como um candidato
chamado `direct` (nome reservado - um relay real não pode usar esse
nome). Se nenhum relay configurado bater a rota direta, `monitor`
desconecta qualquer túnel ativo em vez de manter um relay perdedor só
porque era o melhor dos configurados.
