import { parseGraph } from "./model";
import type { GraphSubmission, NodeInfo } from "./types/desktop";
import type {
  CandidateFeedback,
  CandidateState,
  ExperimentCandidate,
  ExperimentParameter,
  ExperimentProposal,
  ExperimentRecord,
  ExperimentRound,
  ExperimentSpec,
} from "./types/experiment";
import { cloneSubmission } from "./workflow-model";

const object = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const name = (value: unknown): value is string =>
  typeof value === "string" && value.trim().length > 0 && !value.includes("\0");
const finite = (value: unknown): value is number =>
  typeof value === "number" && Number.isFinite(value);
function fail(message: string): never {
  throw new Error(message);
}
const states = new Set<CandidateState>([
  "planned",
  "starting",
  "running",
  "succeeded",
  "failed",
  "cancelled",
  "interrupted",
]);

function safeId(value: unknown, kind: string): string {
  if (!name(value) || !/^[a-zA-Z0-9_-]{1,80}$/.test(value))
    fail(`${kind} ID 无效。`);
  return value;
}

function assertNoSecret(value: unknown): void {
  if (Array.isArray(value)) {
    value.forEach(assertNoSecret);
  } else if (object(value)) {
    for (const [key, child] of Object.entries(value)) {
      if (/^(api.?key|authorization|password|secret|token)$/i.test(key))
        fail("实验记录不能包含模型密钥或凭据。");
      assertNoSecret(child);
    }
  }
}

export function getTunableParameters(
  base: GraphSubmission,
  nodes: NodeInfo[],
): ExperimentParameter[] {
  if (base.mode !== "offline") return [];
  const catalog = new Map(nodes.map((node) => [node.typeId, node]));
  return base.graph.nodes.flatMap((node) => {
    const schema = catalog.get(node.type);
    return (schema?.parameters ?? [])
      .filter((parameter) => {
        const type = parameter.type.toLowerCase();
        return (
          (type === "number" ||
            type === "float" ||
            type === "double" ||
            type === "integer" ||
            type === "int") &&
          finite(parameter.minimum) &&
          finite(parameter.maximum) &&
          parameter.minimum <= parameter.maximum &&
          finite(node.parameters?.[parameter.id])
        );
      })
      .map((parameter) => ({
        node_id: node.id,
        parameter_id: parameter.id,
        minimum: parameter.minimum!,
        maximum: parameter.maximum!,
        integer_only:
          !!parameter.integer_only ||
          ["integer", "int"].includes(parameter.type.toLowerCase()),
      }));
  });
}

export function validateExperimentSpec(
  value: unknown,
  nodes?: NodeInfo[],
): ExperimentSpec {
  if (!object(value) || !name(value.goal) || value.goal.length > 4000)
    fail("请填写不超过 4000 字的实验目标。");
  if (
    !object(value.base) ||
    value.base.mode !== "offline" ||
    !object(value.base.graph) ||
    !object(value.base.options) ||
    Object.keys(value.base.options).length !== 0
  )
    fail("实验仅支持无额外执行选项的整段离线 Graph。");
  const graph = parseGraph(JSON.stringify(value.base.graph));
  const inputs = graph.nodes.filter((node) => node.type === "wav_input");
  const outputs = graph.nodes.filter((node) => node.type === "wav_output");
  if (inputs.length !== 1 || outputs.length !== 1)
    fail("实验需要恰好一个 wav_input 和一个 wav_output。");
  if (!name(inputs[0].parameters?.path) || !name(outputs[0].parameters?.path))
    fail("输入和输出 WAV 路径不能为空。");
  if (
    !Array.isArray(value.parameters) ||
    value.parameters.length < 1 ||
    value.parameters.length > 4
  )
    fail("请选择 1～4 个可调参数。");
  const seen = new Set<string>();
  const catalog = nodes
    ? new Map(nodes.map((node) => [node.typeId, node]))
    : null;
  const parameters: ExperimentParameter[] = value.parameters.map((item) => {
    if (
      !object(item) ||
      !name(item.node_id) ||
      !name(item.parameter_id) ||
      !finite(item.minimum) ||
      !finite(item.maximum) ||
      item.minimum > item.maximum ||
      typeof item.integer_only !== "boolean"
    )
      fail("可调参数定义无效。");
    const node = graph.nodes.find((entry) => entry.id === item.node_id);
    if (!node || !finite(node.parameters?.[item.parameter_id]))
      fail("可调参数在 Graph 中不存在或不是数值。");
    const key = `${item.node_id}\0${item.parameter_id}`;
    if (seen.has(key)) fail("可调参数不能重复。");
    seen.add(key);
    if (
      item.integer_only &&
      (!Number.isInteger(item.minimum) || !Number.isInteger(item.maximum))
    )
      fail("整数参数边界必须是整数。");
    if (catalog) {
      const definition = catalog
        .get(node.type)
        ?.parameters?.find((entry) => entry.id === item.parameter_id);
      if (
        !definition ||
        definition.type !== "number" ||
        catalog.get(node.type)?.execution_domain !== "synchronous" ||
        (finite(definition.minimum) && item.minimum < definition.minimum) ||
        (finite(definition.maximum) && item.maximum > definition.maximum) ||
        !!definition.integer_only !== item.integer_only
      )
        fail("可调范围超出节点 Schema。");
    }
    return {
      node_id: item.node_id,
      parameter_id: item.parameter_id,
      minimum: item.minimum,
      maximum: item.maximum,
      integer_only: item.integer_only,
    };
  });
  if (catalog) {
    for (const node of graph.nodes) {
      for (const entry of catalog.get(node.type)?.parameters ?? []) {
        if (
          (entry.type === "file_path" || entry.type === "FilePath") &&
          !(node.type === "wav_input" && entry.id === "path") &&
          !(node.type === "wav_output" && entry.id === "path")
        )
          fail("实验 Graph 不能包含其他文件路径参数。");
      }
    }
  }
  return {
    goal: value.goal.trim(),
    base: cloneSubmission(value.base as GraphSubmission),
    parameters,
  };
}

export function validateExperimentProposal(
  value: unknown,
  parameters: ExperimentParameter[],
): ExperimentProposal {
  if (
    !object(value) ||
    !Array.isArray(value.candidates) ||
    value.candidates.length < 2 ||
    value.candidates.length > 4
  )
    fail("每批需要 2～4 个候选。");
  if (Object.keys(value).some((key) => key !== "candidates"))
    fail("候选提案只能包含 candidates。");
  const candidates = value.candidates.map((item) => {
    if (
      !object(item) ||
      !name(item.label) ||
      new TextEncoder().encode(item.label).length > 128 ||
      !Array.isArray(item.values) ||
      item.values.length !== parameters.length ||
      Object.keys(item).some((key) => key !== "label" && key !== "values")
    )
      fail("候选标签或数值数量无效。");
    const values = item.values.map((number, index) => {
      const parameter = parameters[index];
      if (
        !finite(number) ||
        number < parameter.minimum ||
        number > parameter.maximum ||
        (parameter.integer_only && !Number.isInteger(number))
      )
        fail("候选数值超出范围或违反整数规则。");
      return number;
    });
    return { label: item.label.trim(), values };
  });
  return { candidates };
}

export function candidateOutputPath(
  recordId: string,
  roundId: string,
  candidateId: string,
): string {
  return `.audio-experiments/${safeId(recordId, "实验")}/${safeId(roundId, "轮次")}-${safeId(candidateId, "候选")}.wav`;
}

export function appendExperimentRound(
  record: ExperimentRecord,
  proposal: ExperimentProposal,
): ExperimentRecord {
  const safe = validateExperimentProposal(proposal, record.parameters);
  if (record.rounds.length >= 20) fail("每个实验最多 20 批候选。");
  const roundId = `r${record.rounds.length + 1}`;
  const candidates: ExperimentCandidate[] = safe.candidates.map(
    (item, index) => {
      const id = `c${index + 1}`;
      return {
        id,
        label: item.label,
        values: [...item.values],
        state: "planned",
        output_path: candidateOutputPath(record.id, roundId, id),
      };
    },
  );
  return { ...record, rounds: [...record.rounds, { id: roundId, candidates }] };
}

export function buildCandidateSubmission(
  record: ExperimentRecord,
  round: ExperimentRound,
  candidate: ExperimentCandidate,
): GraphSubmission {
  if (
    candidate.output_path !==
    candidateOutputPath(record.id, round.id, candidate.id)
  )
    fail("候选输出路径与实验记录不匹配。");
  validateExperimentProposal(
    {
      candidates: [
        { label: candidate.label, values: candidate.values },
        { label: candidate.label, values: candidate.values },
      ],
    },
    record.parameters,
  );
  const graph = structuredClone(record.base.graph);
  for (let index = 0; index < record.parameters.length; index++) {
    const parameter = record.parameters[index];
    const node = graph.nodes.find((entry) => entry.id === parameter.node_id)!;
    node.parameters = {
      ...node.parameters,
      [parameter.parameter_id]: candidate.values[index],
    };
  }
  const input = graph.nodes.find((node) => node.id === record.input.node_id)!;
  const output = graph.nodes.find((node) => node.id === record.output_node_id)!;
  input.parameters = { ...input.parameters, path: record.input.snapshot_path };
  output.parameters = { ...output.parameters, path: candidate.output_path };
  return cloneSubmission({ mode: "offline", graph, options: {} });
}

export function updateExperimentCandidate(
  record: ExperimentRecord,
  roundId: string,
  candidateId: string,
  change: (candidate: ExperimentCandidate) => ExperimentCandidate,
): ExperimentRecord {
  let found = false;
  const rounds = record.rounds.map((round) =>
    round.id !== roundId
      ? round
      : {
          ...round,
          candidates: round.candidates.map((candidate) => {
            if (candidate.id !== candidateId) return candidate;
            found = true;
            return change(candidate);
          }),
        },
  );
  if (!found) fail("候选不存在。");
  return { ...record, rounds };
}

export function normalizeExperimentRecord(
  value: unknown,
  interruptUnfinished = true,
): { record: ExperimentRecord; interrupted: boolean } {
  assertNoSecret(value);
  if (
    !object(value) ||
    value.schema_version !== 1 ||
    !name(value.id) ||
    !name(value.workspace) ||
    !finite(value.created_at) ||
    !object(value.input) ||
    !name(value.input.node_id) ||
    !name(value.input.original_path) ||
    !name(value.input.snapshot_path) ||
    !name(value.output_node_id) ||
    !Array.isArray(value.rounds)
  )
    fail("实验记录格式无效。");
  const id = safeId(value.id, "实验");
  if (
    Object.keys(value).some(
      (key) =>
        ![
          "schema_version",
          "id",
          "workspace",
          "created_at",
          "input",
          "output_node_id",
          "goal",
          "base",
          "parameters",
          "rounds",
        ].includes(key),
    )
  )
    fail("实验记录包含未知字段。");
  if (
    Object.keys(value.input).some(
      (key) => !["node_id", "original_path", "snapshot_path"].includes(key),
    )
  )
    fail("实验输入记录包含未知字段。");
  const spec = validateExperimentSpec(value);
  const inputDetails = value.input as Record<string, unknown>;
  const input = spec.base.graph.nodes.find(
    (node) => node.id === inputDetails.node_id && node.type === "wav_input",
  );
  const output = spec.base.graph.nodes.find(
    (node) => node.id === value.output_node_id && node.type === "wav_output",
  );
  if (
    !input ||
    !output ||
    input.parameters?.path !== inputDetails.snapshot_path
  )
    fail("实验输入快照与 Graph 基线不匹配。");
  if (value.rounds.length > 20) fail("实验轮次数量超出上限。");
  let interrupted = false;
  const rounds: ExperimentRound[] = value.rounds.map((item, roundIndex) => {
    if (
      !object(item) ||
      item.id !== `r${roundIndex + 1}` ||
      !Array.isArray(item.candidates) ||
      item.candidates.length < 2 ||
      item.candidates.length > 4
    )
      fail("实验轮次格式无效。");
    if (Object.keys(item).some((key) => key !== "id" && key !== "candidates"))
      fail("实验轮次包含未知字段。");
    const candidates: ExperimentCandidate[] = item.candidates.map(
      (raw, candidateIndex) => {
        if (
          !object(raw) ||
          raw.id !== `c${candidateIndex + 1}` ||
          !name(raw.label) ||
          new TextEncoder().encode(raw.label).length > 128 ||
          !states.has(raw.state as CandidateState) ||
          raw.output_path !==
            candidateOutputPath(id, item.id as string, raw.id as string)
        )
          fail("实验候选格式无效。");
        if (
          Object.keys(raw).some(
            (key) =>
              ![
                "id",
                "label",
                "values",
                "state",
                "output_path",
                "task_id",
                "result",
                "errors",
                "feedback",
              ].includes(key),
          )
        )
          fail("实验候选包含未知字段。");
        validateExperimentProposal(
          {
            candidates: [
              { label: raw.label, values: raw.values },
              { label: raw.label, values: raw.values },
            ],
          },
          spec.parameters,
        );
        if (raw.task_id !== undefined && !name(raw.task_id))
          fail("任务 ID 无效。");
        if (
          raw.feedback !== undefined &&
          (!object(raw.feedback) ||
            !["preferred", "acceptable", "rejected"].includes(
              raw.feedback.rating as string,
            ) ||
            typeof raw.feedback.note !== "string" ||
            new TextEncoder().encode(raw.feedback.note).length > 4096)
        )
          fail("评价格式无效。");
        const state = raw.state as CandidateState;
        if (
          state === "planned" &&
          (raw.task_id !== undefined ||
            raw.result !== undefined ||
            raw.errors !== undefined ||
            raw.feedback !== undefined)
        )
          fail("未运行候选包含任务结果。");
        if (
          state === "starting" &&
          (raw.task_id !== undefined || raw.result !== undefined)
        )
          fail("启动中候选包含任务结果。");
        if (
          ["running", "succeeded", "cancelled"].includes(state) &&
          raw.task_id === undefined
        )
          fail("运行候选缺少任务 ID。");
        if (state === "succeeded" && raw.result === undefined)
          fail("成功候选缺少任务结果。");
        if (raw.feedback !== undefined && state !== "succeeded")
          fail("只能评价成功候选。");
        if (state === "starting" || state === "running") interrupted = true;
        return {
          id: raw.id as string,
          label: raw.label as string,
          values: [...(raw.values as number[])],
          output_path: raw.output_path as string,
          state:
            interruptUnfinished && (state === "starting" || state === "running")
              ? "interrupted"
              : state,
          ...(raw.task_id === undefined
            ? {}
            : { task_id: raw.task_id as string }),
          ...(raw.result === undefined
            ? {}
            : { result: structuredClone(raw.result) }),
          ...(raw.errors === undefined
            ? {}
            : { errors: structuredClone(raw.errors) }),
          ...(raw.feedback === undefined
            ? {}
            : {
                feedback: {
                  rating: raw.feedback.rating,
                  note: raw.feedback.note,
                } as CandidateFeedback,
              }),
        };
      },
    );
    return { id: item.id as string, candidates };
  });
  return {
    record: {
      ...spec,
      schema_version: 1,
      id,
      workspace: value.workspace as string,
      created_at: value.created_at as number,
      input: {
        node_id: inputDetails.node_id as string,
        original_path: inputDetails.original_path as string,
        snapshot_path: inputDetails.snapshot_path as string,
      },
      output_node_id: value.output_node_id as string,
      rounds,
    },
    interrupted,
  };
}
