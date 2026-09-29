import type { GraphSubmission } from "./desktop";

export type ExperimentParameter = {
  node_id: string;
  parameter_id: string;
  minimum: number;
  maximum: number;
  integer_only: boolean;
};
export type ExperimentSpec = {
  goal: string;
  base: GraphSubmission;
  parameters: ExperimentParameter[];
};
export type CandidateState =
  | "planned"
  | "starting"
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "interrupted";
export type CandidateFeedback = {
  rating: "preferred" | "acceptable" | "rejected";
  note: string;
};
export type ExperimentCandidate = {
  id: string;
  label: string;
  values: number[];
  state: CandidateState;
  output_path: string;
  task_id?: string;
  result?: unknown;
  errors?: unknown;
  feedback?: CandidateFeedback;
};
export type ExperimentRound = { id: string; candidates: ExperimentCandidate[] };
export type ExperimentRecord = ExperimentSpec & {
  schema_version: 1;
  id: string;
  workspace: string;
  created_at: number;
  input: { node_id: string; original_path: string; snapshot_path: string };
  output_node_id: string;
  rounds: ExperimentRound[];
};
export type ExperimentSummary = {
  id: string;
  goal: string;
  created_at: number;
};
export type ExperimentProposal = {
  candidates: { label: string; values: number[] }[];
};
export type ExperimentAiReply = {
  request_id: string;
  proposal?: ExperimentProposal;
  text: string;
};
