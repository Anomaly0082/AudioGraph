import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";

type PortDescriptor = {
  id: string;
  type: string;
};

type NodeDescriptor = {
  typeId: string;
  displayName: string;
  inputs: PortDescriptor[];
  outputs: PortDescriptor[];
};

type NodeList = {
  nodes: NodeDescriptor[];
};

type RunResult = {
  success: boolean;
  peak: number;
  gainDb: number;
};

function GraphPreview({ gainDb }: { gainDb: number }) {
  return (
    <section className="panel graph-panel">
      <div className="panel-heading">
        <div>
          <p className="eyebrow">COMPILED GRAPH</p>
          <h2>同步数据流</h2>
        </div>
        <span className="status-dot">Ready</span>
      </div>

      <div className="graph">
        <div className="node source">
          <span>FilePath</span>
          <strong>WAV Input</strong>
          <small>输出 Audio</small>
        </div>
        <div className="arrow">→</div>
        <div className="node process">
          <span>Audio → Audio</span>
          <strong>Gain</strong>
          <small>{gainDb.toFixed(1)} dB</small>
        </div>
        <div className="branch">
          <div className="branch-row">
            <div className="arrow">→</div>
            <div className="node sink">
              <span>Audio → FilePath</span>
              <strong>WAV Output</strong>
              <small>写入 PCM16</small>
            </div>
          </div>
          <div className="branch-row">
            <div className="arrow">↘</div>
            <div className="node metric">
              <span>Audio → Number</span>
              <strong>Peak Meter</strong>
              <small>计算绝对峰值</small>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

export default function App() {
  const [inputPath, setInputPath] = useState("");
  const [outputPath, setOutputPath] = useState("");
  const [gainDb, setGainDb] = useState(0);
  const [nodes, setNodes] = useState<NodeDescriptor[]>([]);
  const [result, setResult] = useState<RunResult | null>(null);
  const [error, setError] = useState("");
  const [running, setRunning] = useState(false);

  useEffect(() => {
    invoke<NodeList>("list_nodes")
      .then((value) => setNodes(value.nodes))
      .catch((reason) => setError(String(reason)));
  }, []);

  async function chooseInput() {
    const selected = await open({
      multiple: false,
      filters: [{ name: "PCM WAV", extensions: ["wav"] }],
    });
    if (typeof selected === "string") {
      setInputPath(selected);
      if (!outputPath) {
        setOutputPath(selected.replace(/\.wav$/i, "-processed.wav"));
      }
    }
  }

  async function chooseOutput() {
    const selected = await save({
      defaultPath: outputPath || "processed.wav",
      filters: [{ name: "PCM WAV", extensions: ["wav"] }],
    });
    if (selected) {
      setOutputPath(selected);
    }
  }

  async function runGraph() {
    setRunning(true);
    setError("");
    setResult(null);
    try {
      const value = await invoke<RunResult>("run_demo_graph", {
        inputPath,
        outputPath,
        gainDb,
      });
      setResult(value);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setRunning(false);
    }
  }

  return (
    <main>
      <header>
        <div>
          <p className="eyebrow">AUDIOPROCESS / P0</p>
          <h1>Typed Graph Executor</h1>
          <p className="subtitle">Tauri → Rust → C++ 同步节点图实验</p>
        </div>
        <div className="capability-count">
          <strong>{nodes.length}</strong>
          <span>discovered nodes</span>
        </div>
      </header>

      <GraphPreview gainDb={gainDb} />

      <section className="panel controls">
        <div className="panel-heading">
          <div>
            <p className="eyebrow">EXECUTION</p>
            <h2>运行原型 Graph</h2>
          </div>
        </div>

        <label>
          <span>输入 WAV</span>
          <div className="file-row">
            <input value={inputPath} onChange={(event) => setInputPath(event.target.value)} />
            <button className="secondary" onClick={chooseInput}>选择</button>
          </div>
        </label>

        <label>
          <span>输出 WAV</span>
          <div className="file-row">
            <input value={outputPath} onChange={(event) => setOutputPath(event.target.value)} />
            <button className="secondary" onClick={chooseOutput}>选择</button>
          </div>
        </label>

        <label>
          <span>Gain：{gainDb.toFixed(1)} dB</span>
          <input
            type="range"
            min="-24"
            max="12"
            step="0.5"
            value={gainDb}
            onChange={(event) => setGainDb(Number(event.target.value))}
          />
        </label>

        <button
          className="primary"
          disabled={running || !inputPath || !outputPath}
          onClick={runGraph}
        >
          {running ? "执行中…" : "编译并执行 Graph"}
        </button>

        {result && (
          <div className="result success">
            <strong>执行成功</strong>
            <span>Peak：{result.peak.toFixed(6)}</span>
            <span>Gain：{result.gainDb.toFixed(1)} dB</span>
          </div>
        )}
        {error && <div className="result error">{error}</div>}
      </section>

      <section className="panel discovered">
        <div className="panel-heading">
          <div>
            <p className="eyebrow">CAPABILITY DISCOVERY</p>
            <h2>C++ NodeRegistry</h2>
          </div>
        </div>
        <div className="node-list">
          {nodes.map((node) => (
            <div className="node-summary" key={node.typeId}>
              <strong>{node.displayName}</strong>
              <code>{node.typeId}</code>
              <span>
                {node.inputs.map((port) => `${port.id}:${port.type}`).join(", ") || "无输入"}
                {" → "}
                {node.outputs.map((port) => `${port.id}:${port.type}`).join(", ") || "无输出"}
              </span>
            </div>
          ))}
        </div>
      </section>
    </main>
  );
}

