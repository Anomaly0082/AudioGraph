import type { GraphDraft } from "../hooks/useGraphDraft";
import type { AudioSession } from "../hooks/useAudioSession";
import type { Experiments } from "../hooks/useExperiments";
import type {
  CandidateFeedback,
  CandidateState,
  ExperimentParameter,
} from "../types/experiment";
import { formatError } from "../model";
import { resultFields } from "../presentation";
import Disclosure from "../components/Disclosure";

export type ExperimentSetup = {
  goal: string;
  parameters: Record<string, { minimum: string; maximum: string }>;
  manual: string;
  notes: Record<string, string>;
  ratings: Record<string, CandidateFeedback["rating"]>;
};
type Props = {
  experiments: Experiments;
  session: AudioSession;
  draft: GraphDraft;
  setup: ExperimentSetup;
  onSetup: (value: ExperimentSetup) => void;
  blocked: boolean;
  onEditor: () => void;
  onSettings: () => void;
  onCopy: (value: string) => void;
};
const labels: Record<CandidateState, string> = {
  planned: "待运行",
  starting: "正在提交",
  running: "处理中",
  succeeded: "已完成",
  failed: "失败",
  cancelled: "已取消",
  interrupted: "已中断",
};
const ratings: Record<CandidateFeedback["rating"], string> = {
  preferred: "更符合需求",
  acceptable: "可接受",
  rejected: "不符合",
};

export default function ExperimentsPage({
  experiments: ex,
  session,
  draft,
  setup,
  onSetup,
  blocked,
  onEditor,
  onSettings,
  onCopy,
}: Props) {
  const record = ex.record;
  const locked = ex.busy || blocked || !session.connection;
  const catalog = session.connection?.capabilities.nodes ?? [];
  const options = (draft.localGraph.graph?.nodes ?? []).flatMap((node) => {
    const info = catalog.find((item) => item.typeId === node.type);
    return (info?.parameters ?? [])
      .filter((parameter) => parameter.type === "number")
      .map((parameter) => ({
        node,
        parameter,
        key: JSON.stringify([node.id, parameter.id]),
      }));
  });
  const selected = options.filter((item) =>
    Object.hasOwn(setup.parameters, item.key),
  );
  function act(action: () => Promise<unknown> | unknown) {
    void Promise.resolve()
      .then(action)
      .catch((reason) => ex.setError(formatError(reason)));
  }
  function create() {
    const parameters: ExperimentParameter[] = selected.map(
      ({ node, parameter, key }) => {
        const range = setup.parameters[key];
        if (!range.minimum.trim() || !range.maximum.trim())
          throw new Error("请填写每个参数的上下限。");
        return {
          node_id: node.id,
          parameter_id: parameter.id,
          minimum: Number(range.minimum),
          maximum: Number(range.maximum),
          integer_only: !!parameter.integer_only,
        };
      },
    );
    return ex.create({
      goal: setup.goal,
      base: draft.buildSubmission(),
      parameters,
    });
  }
  function addManual() {
    const rows: unknown = JSON.parse(setup.manual);
    if (!Array.isArray(rows) || rows.length < 2 || rows.length > 4)
      throw new Error("请输入2～4行参数数组。");
    return ex.addManual({
      candidates: rows.map((values, index) => {
        if (
          !Array.isArray(values) ||
          values.some(
            (value) => typeof value !== "number" || !Number.isFinite(value),
          )
        )
          throw new Error("候选值必须是数值数组。");
        return { label: `方案 ${index + 1}`, values };
      }),
    });
  }
  return (
    <div className="page-stack experiments-page">
      <section className="panel">
        <div className="section-heading">
          <h2>实验记录</h2>
          <button disabled={locked} onClick={() => act(ex.refresh)}>
            刷新
          </button>
        </div>
        <select
          aria-label="选择实验记录"
          value={record?.id ?? ""}
          disabled={locked}
          onChange={(event) => {
            if (event.target.value) act(() => ex.load(event.target.value));
          }}
        >
          <option value="">选择已保存的实验</option>
          {ex.history.map((item) => (
            <option key={item.id} value={item.id}>
              {item.goal} · {new Date(item.created_at).toLocaleString()}
            </option>
          ))}
        </select>
        {!session.connection && (
          <p className="hint">先打开音频所在的工作区。</p>
        )}
      </section>
      {ex.error && (
        <div className="banner error" role="alert">
          {ex.error}
          <button onClick={() => ex.setError("")}>关闭</button>
        </div>
      )}
      {ex.notice && (
        <div className="banner info" role="status">
          {ex.notice}
        </div>
      )}
      <Disclosure
        label="实验创建选项"
        className="panel experiment-create"
        open={!record}
      >
        <p className="hint">
          整段离线 WAV，固定输入和节点连接，只调整选中的数值参数。
        </p>
        <div className="action-row">
          <button onClick={onEditor}>编辑 Graph</button>
        </div>
        <label>
          比较目标
          <input
            value={setup.goal}
            maxLength={4000}
            placeholder="例如：语音足够响亮，同时避免削波"
            disabled={locked}
            onChange={(event) =>
              onSetup({ ...setup, goal: event.target.value })
            }
          />
        </label>
        <div className="experiment-parameters">
          {options.map(({ node, parameter, key }) => {
            const checked = Object.hasOwn(setup.parameters, key);
            return (
              <div className="experiment-parameter" key={key}>
                <label className="checkbox-label">
                  <input
                    type="checkbox"
                    checked={checked}
                    disabled={locked || (!checked && selected.length >= 4)}
                    onChange={(event) => {
                      const parameters = { ...setup.parameters };
                      if (event.target.checked)
                        parameters[key] = {
                          minimum:
                            parameter.minimum === undefined
                              ? ""
                              : String(parameter.minimum),
                          maximum:
                            parameter.maximum === undefined
                              ? ""
                              : String(parameter.maximum),
                        };
                      else delete parameters[key];
                      onSetup({ ...setup, parameters });
                    }}
                  />
                  {node.id}.{parameter.id}
                  {parameter.unit ? ` (${parameter.unit})` : ""}
                </label>
                {checked && (
                  <div className="experiment-range">
                    <label>
                      最小
                      <input
                        type="number"
                        step={parameter.integer_only ? 1 : "any"}
                        value={setup.parameters[key].minimum}
                        disabled={locked}
                        onChange={(event) =>
                          onSetup({
                            ...setup,
                            parameters: {
                              ...setup.parameters,
                              [key]: {
                                ...setup.parameters[key],
                                minimum: event.target.value,
                              },
                            },
                          })
                        }
                      />
                    </label>
                    <label>
                      最大
                      <input
                        type="number"
                        step={parameter.integer_only ? 1 : "any"}
                        value={setup.parameters[key].maximum}
                        disabled={locked}
                        onChange={(event) =>
                          onSetup({
                            ...setup,
                            parameters: {
                              ...setup.parameters,
                              [key]: {
                                ...setup.parameters[key],
                                maximum: event.target.value,
                              },
                            },
                          })
                        }
                      />
                    </label>
                  </div>
                )}
              </div>
            );
          })}
          {!options.length && (
            <p className="hint">
              当前草稿没有可选的数值参数，可先载入音量调整模板。
            </p>
          )}
        </div>
        <div className="action-row">
          <button
            className="primary"
            disabled={
              locked ||
              !!session.task ||
              !setup.goal.trim() ||
              selected.length < 1 ||
              selected.length > 4 ||
              draft.mode !== "offline"
            }
            onClick={() => act(create)}
          >
            创建实验并复制输入
          </button>
        </div>
        <p className="hint">
          音频副本、结果和记录保存在工作区的 .audio-experiments
          目录。单个输入上限256 MiB。
        </p>
      </Disclosure>
      {record && (
        <>
          <section className="panel">
            <div className="section-heading">
              <h2>{record.goal}</h2>
              <code>{record.id}</code>
            </div>
            <p className="muted">
              输入副本：<code>{record.input.snapshot_path}</code>
            </p>
            <div className="experiment-bindings">
              {record.parameters.map((parameter, index) => (
                <div key={index}>
                  <code>
                    {parameter.node_id}.{parameter.parameter_id}
                  </code>
                  <small>
                    {parameter.minimum} ～ {parameter.maximum}
                    {parameter.integer_only ? " · 整数" : ""}
                  </small>
                </div>
              ))}
            </div>
            <div className="action-row">
              <button
                className="primary"
                disabled={locked || !!session.task || !!ex.pendingProposal}
                onClick={() => act(ex.propose)}
              >
                AI 建议下一批参数
              </button>
              <button disabled={ex.busy} onClick={onSettings}>
                模型设置
              </button>
              {ex.canStop && (
                <button className="danger" onClick={() => act(ex.stop)}>
                  {ex.running ? "停止实验" : "停止 AI 请求"}
                </button>
              )}
            </div>
            <p className="hint">
              AI会收到本实验的配置、结果和人工评价，不发送音频样本。每次点击只请求一次。
            </p>
            <Disclosure label="手动候选输入">
              <label>
                每行按上方参数顺序填写一组数值（JSON）
                <textarea
                  className="experiment-manual"
                  value={setup.manual}
                  disabled={locked || !!ex.pendingProposal}
                  onChange={(event) =>
                    onSetup({ ...setup, manual: event.target.value })
                  }
                />
              </label>
              <button
                disabled={locked || !!ex.pendingProposal}
                onClick={() => act(addManual)}
              >
                保存候选
              </button>
            </Disclosure>
          </section>
          {ex.pendingProposal && (
            <section className="panel">
              <h2>待确认候选</h2>
              <div className="experiment-proposals">
                {ex.pendingProposal.candidates.map((candidate, index) => (
                  <p key={index}>
                    {candidate.label}：
                    <code>{candidate.values.join(" / ")}</code>
                  </p>
                ))}
              </div>
              <div className="action-row">
                <button
                  disabled={locked}
                  className="primary"
                  onClick={() => act(ex.acceptProposal)}
                >
                  保存这一批
                </button>
                <button disabled={locked} onClick={ex.rejectProposal}>
                  丢弃
                </button>
              </div>
            </section>
          )}
          {record.rounds.map((round) => (
            <section className="panel" key={round.id}>
              <div className="section-heading">
                <h2>批次 {round.id}</h2>
                <button
                  className="primary"
                  disabled={
                    locked ||
                    !!session.task ||
                    round.candidates.some(
                      (candidate) => candidate.state !== "planned",
                    )
                  }
                  onClick={() => act(() => ex.runRound(round.id))}
                >
                  确认并运行这一批
                </button>
              </div>
              <div className="experiment-candidates">
                {round.candidates.map((candidate) => {
                  const key = JSON.stringify([
                    record.id,
                    round.id,
                    candidate.id,
                  ]);
                  const fields = resultFields(candidate.result).filter(
                    (field) => !field.file,
                  );
                  const rating =
                    setup.ratings[key] ??
                    candidate.feedback?.rating ??
                    "acceptable";
                  const note =
                    setup.notes[key] ?? candidate.feedback?.note ?? "";
                  return (
                    <article
                      className="experiment-candidate"
                      key={candidate.id}
                    >
                      <div className="section-heading">
                        <h3>{candidate.label}</h3>
                        <span
                          className={`badge ${candidate.state === "succeeded" ? "good" : ["failed", "interrupted"].includes(candidate.state) ? "bad" : ""}`}
                        >
                          {!ex.running &&
                          ["starting", "running"].includes(candidate.state)
                            ? "未确认完成"
                            : labels[candidate.state]}
                        </span>
                      </div>
                      <dl>
                        {record.parameters.map((parameter, index) => (
                          <div key={index}>
                            <dt>
                              {parameter.node_id}.{parameter.parameter_id}
                            </dt>
                            <dd>{candidate.values[index]}</dd>
                          </div>
                        ))}
                      </dl>
                      <code className="experiment-output">
                        {candidate.output_path}
                      </code>
                      {candidate.state === "succeeded" && (
                        <div className="action-row">
                          <button
                            onClick={() =>
                              onCopy(
                                `${record.workspace}/${candidate.output_path}`,
                              )
                            }
                          >
                            复制输出路径
                          </button>
                          <button
                            disabled={locked}
                            onClick={() =>
                              act(() => ex.restore(round.id, candidate.id))
                            }
                          >
                            恢复到编辑器
                          </button>
                        </div>
                      )}
                      {!!fields.length && (
                        <Disclosure label="处理指标">
                          <pre>{JSON.stringify(fields, null, 2)}</pre>
                        </Disclosure>
                      )}
                      {candidate.errors != null && (
                        <pre className="inline-error">
                          {formatError(candidate.errors)}
                        </pre>
                      )}
                      {candidate.state === "succeeded" && (
                        <div className="experiment-feedback">
                          <label>
                            试听评价
                            <select
                              value={rating}
                              disabled={locked}
                              onChange={(event) =>
                                onSetup({
                                  ...setup,
                                  ratings: {
                                    ...setup.ratings,
                                    [key]: event.target
                                      .value as CandidateFeedback["rating"],
                                  },
                                })
                              }
                            >
                              {Object.entries(ratings).map(([value, label]) => (
                                <option key={value} value={value}>
                                  {label}
                                </option>
                              ))}
                            </select>
                          </label>
                          <label>
                            备注
                            <input
                              value={note}
                              maxLength={1000}
                              disabled={locked}
                              onChange={(event) =>
                                onSetup({
                                  ...setup,
                                  notes: {
                                    ...setup.notes,
                                    [key]: event.target.value,
                                  },
                                })
                              }
                            />
                          </label>
                          <button
                            disabled={locked}
                            onClick={() =>
                              act(() =>
                                ex.rate(round.id, candidate.id, {
                                  rating,
                                  note,
                                }),
                              )
                            }
                          >
                            保存评价
                          </button>
                          {candidate.feedback && (
                            <small>
                              已保存：{ratings[candidate.feedback.rating]}
                              {candidate.feedback.note
                                ? ` · ${candidate.feedback.note}`
                                : ""}
                            </small>
                          )}
                        </div>
                      )}
                    </article>
                  );
                })}
              </div>
            </section>
          ))}
        </>
      )}
    </div>
  );
}
