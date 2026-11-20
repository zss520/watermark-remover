import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import {
  COMPLIANCE,
  VERDICT_LABEL,
  base64ToBlob,
  base64ToBytes,
  base64ToObjectUrl,
  bytesToBase64,
  fetchSample,
  detectCameraTime,
  fetchTemplates,
  fileToBase64,
  processImage,
  type CameraTimeRequest,
  type Probe,
  type Report,
  type TemplateBundle,
} from "./api";

interface BatchItem {
  name: string;
  verdict: string;
}

type Box = [number, number, number, number];

const EMPTY_PROBE = "把指针移到图上查看处理前后的 RGB。滚轮缩放。按住空格，结果画布临时显示原图。";

export function App() {
  const [accepted, setAccepted] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [dropping, setDropping] = useState(false);
  const [error, setError] = useState("");
  const [note, setNote] = useState("正在查找本机的豆包模板。");
  const [jpeg, setJpeg] = useState(false);
  const [keepExif, setKeepExif] = useState(false);
  const [manual, setManual] = useState(false);
  const [editTime, setEditTime] = useState(false);
  const [year, setYear] = useState(2026);
  const [month, setMonth] = useState(1);
  const [day, setDay] = useState(1);
  const [hour, setHour] = useState(0);
  const [minute, setMinute] = useState(0);
  const [second, setSecond] = useState(0);
  const [hasSecond, setHasSecond] = useState(false);
  const [timeNote, setTimeNote] = useState("勾选后会识别图中的相机时间，可改年月日时分秒。");
  const [zoom, setZoom] = useState(1);
  const [imageName, setImageName] = useState("image.png");
  const [imageBase64, setImageBase64] = useState("");
  const [templates, setTemplates] = useState<TemplateBundle[]>([]);
  const [report, setReport] = useState<Report | null>(null);
  const [probe, setProbe] = useState<Probe | null>(null);
  const [resultBase64, setResultBase64] = useState<string | null>(null);
  const [holdOriginal, setHoldOriginal] = useState(false);
  const [probeText, setProbeText] = useState(EMPTY_PROBE);
  const [batch, setBatch] = useState<BatchItem[]>([]);
  const [bbox, setBbox] = useState<Box | null>(null);
  const [draft, setDraft] = useState<Box | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const originalRef = useRef<HTMLCanvasElement>(null);
  const resultRef = useRef<HTMLCanvasElement>(null);
  const originalWrap = useRef<HTMLDivElement>(null);
  const resultWrap = useRef<HTMLDivElement>(null);
  const dirInput = useRef<HTMLInputElement>(null);
  const drag = useRef<{ x: number; y: number } | null>(null);
  const draftRef = useRef<Box | null>(null);
  const pending = useRef<(() => void) | null>(null);
  const customTemplates = useRef(false);

  useEffect(() => {
    let cancelled = false;
    fetchTemplates()
      .then((list) => {
        if (cancelled || customTemplates.current) return;
        if (list.templates.length === 0) {
          setNote("未选择模板时使用合成示例。它不是豆包或千问的真实水印。");
          return;
        }
        setTemplates(list.templates);
        setNote(list.note);
      })
      .catch(() => {
        if (!cancelled && !customTemplates.current) {
          setNote("未选择模板时使用合成示例。它不是豆包或千问的真实水印。");
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const node = dirInput.current;
    node?.setAttribute("webkitdirectory", "");
    node?.setAttribute("directory", "");
  }, []);

  useEffect(() => {
    const down = (event: KeyboardEvent) => {
      const target = event.target;
      if (target instanceof HTMLInputElement || target instanceof HTMLButtonElement || target instanceof HTMLTextAreaElement) {
        return;
      }
      if (event.code === "Space") {
        event.preventDefault();
        setHoldOriginal(true);
      }
    };
    const up = (event: KeyboardEvent) => {
      if (event.code === "Space") setHoldOriginal(false);
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
    };
  }, []);

  useZoom(originalWrap, setZoom);
  useZoom(resultWrap, setZoom);

  useEffect(() => {
    if (!editTime || !imageBase64) return;
    let cancelled = false;
    detectCameraTime(imageBase64)
      .then((found) => {
        if (cancelled) return;
        if (!found.found || found.year === undefined || found.month === undefined || found.day === undefined || found.hour === undefined || found.minute === undefined) {
          setHasSecond(false);
          setTimeNote("这张图没有识别到相机时间。处理时如果仍找不到，日期保持原样。");
          return;
        }
        setYear(found.year);
        setMonth(found.month);
        setDay(found.day);
        setHour(found.hour);
        setMinute(found.minute);
        setSecond(found.second ?? 0);
        setHasSecond(found.second !== null && found.second !== undefined);
        setTimeNote(
          found.second === null || found.second === undefined
            ? `识别到 ${found.text}。原格式没有秒，不会补上秒。机型那一行不改。`
            : `识别到 ${found.text}。机型那一行不改。`,
        );
      })
      .catch(() => {
        if (!cancelled) setTimeNote("读取相机时间失败。仍可填写，处理时如果找不到就不会改。");
      });
    return () => {
      cancelled = true;
    };
  }, [editTime, imageBase64]);

  useEffect(() => {
    let cancelled = false;
    const url = imageBase64 ? base64ToObjectUrl(imageBase64) : "";
    if (!url) {
      clearCanvas(originalRef.current);
      setSize({ w: 0, h: 0 });
      return;
    }
    const image = new Image();
    image.onload = () => {
      if (cancelled) return;
      drawImage(originalRef.current, image);
      setSize({ w: image.naturalWidth, h: image.naturalHeight });
    };
    image.src = url;
    return () => {
      cancelled = true;
      URL.revokeObjectURL(url);
    };
  }, [imageBase64]);

  useEffect(() => {
    let cancelled = false;
    const source = holdOriginal ? imageBase64 : resultBase64;
    const mime = jpeg && source === resultBase64 ? "image/jpeg" : "image/png";
    const url = source ? base64ToObjectUrl(source, mime) : "";
    if (!url) {
      clearCanvas(resultRef.current);
      return;
    }
    const image = new Image();
    image.onload = () => {
      if (!cancelled) drawImage(resultRef.current, image);
    };
    image.src = url;
    return () => {
      cancelled = true;
      URL.revokeObjectURL(url);
    };
  }, [resultBase64, holdOriginal, imageBase64, jpeg]);

  function ask(action: () => void) {
    pending.current = action;
    setConfirming(true);
  }

  function payloadTemplates(): TemplateBundle[] | undefined {
    return templates.length > 0 ? templates : undefined;
  }

  async function loadSample() {
    customTemplates.current = true;
    setError("");
    setBusy(true);
    try {
      const sample = await fetchSample();
      setImageName("synthetic.png");
      setImageBase64(sample.imageBase64);
      setTemplates([{ templateJson: sample.templateJson, alphaBase64: sample.alphaBase64 }]);
      setReport(null);
      setProbe(null);
      setResultBase64(null);
      setBbox(null);
      setNote(sample.note);
    } catch (err) {
      setError(err instanceof Error ? err.message : "示例加载失败");
    } finally {
      setBusy(false);
    }
  }

  async function onImage(file: File | undefined) {
    if (!file) return;
    setImageName(file.name);
    setImageBase64(await fileToBase64(file));
    setReport(null);
    setProbe(null);
    setResultBase64(null);
    setBbox(null);
    setError("");
  }

  async function onTemplate(jsonFile: File | undefined, alphaFile: File | undefined, biasFile: File | undefined) {
    if (!jsonFile || !alphaFile) {
      setError("请同时选择模板 JSON 和 Alpha PNG");
      return;
    }
    const text = await jsonFile.text();
    let needsBias = false;
    try {
      needsBias = Boolean((JSON.parse(text) as { biasMap?: string }).biasMap);
    } catch {
      setError("模板 JSON 无法解析");
      return;
    }
    if (needsBias && !biasFile) {
      setError("这个模板带仿射偏置，请再选择 Bias PNG");
      return;
    }
    customTemplates.current = true;
    const alpha = new Uint8Array(await alphaFile.arrayBuffer());
    const bias = biasFile ? bytesToBase64(new Uint8Array(await biasFile.arrayBuffer())) : undefined;
    setTemplates([{ templateJson: text, alphaBase64: bytesToBase64(alpha), biasBase64: bias }]);
    setNote(`已载入模板 ${jsonFile.name}`);
    setError("");
  }

  async function onTemplateDir(files: FileList | null) {
    if (!files || files.length === 0) return;
    customTemplates.current = true;
    const list = Array.from(files);
    const bundles: TemplateBundle[] = [];
    for (const json of list.filter((file) => file.name.toLowerCase().endsWith(".json"))) {
      const text = await json.text();
      let alphaName = json.name.replace(/\.json$/i, ".png");
      try {
        const parsed = JSON.parse(text) as { alphaMap?: string; biasMap?: string };
        const mapped = parsed.alphaMap?.split(/[/\\]/).pop();
        if (mapped) alphaName = mapped;
        const biasName = parsed.biasMap?.split(/[/\\]/).pop();
        const alpha = list.find((file) => file.name === alphaName);
        if (!alpha) continue;
        if (biasName && !list.some((file) => file.name === biasName)) {
          setError(`缺少偏置贴图 ${biasName}`);
          return;
        }
        const bias = biasName ? list.find((file) => file.name === biasName) : undefined;
        bundles.push({
          templateJson: text,
          alphaBase64: bytesToBase64(new Uint8Array(await alpha.arrayBuffer())),
          biasBase64: bias ? bytesToBase64(new Uint8Array(await bias.arrayBuffer())) : undefined,
        });
        continue;
      } catch {
        continue;
      }
    }
    if (bundles.length === 0) {
      setError("目录里没有成对的模板 JSON 和 Alpha PNG");
      return;
    }
    setTemplates(bundles);
    setNote(`已载入 ${bundles.length} 个本地模板`);
    setError("");
  }

  function cameraPayload(): CameraTimeRequest | undefined {
    if (!editTime) return undefined;
    return {
      year,
      month,
      day,
      hour,
      minute,
      second: hasSecond ? second : null,
    };
  }

  async function runProcess(acceptLow: boolean) {
    if (!imageBase64) return;
    setBusy(true);
    setError("");
    try {
      const response = await processImage({
        imageBase64,
        templates: payloadTemplates(),
        accept: acceptLow,
        jpeg,
        keepExif,
        bbox: bbox ?? undefined,
        cameraTime: cameraPayload(),
      });
      setReport(response.report);
      setProbe(response.probe);
      setResultBase64(response.imageBase64);
      if (response.report.flags.includes("confirmation_required")) {
        setNote("置信度处于确认区间，还没有写出结果。可以改框，或选择“确认低置信度”。");
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "处理失败");
    } finally {
      setBusy(false);
    }
  }

  async function runBatch(files: FileList | null) {
    if (!files || files.length === 0) return;
    setBusy(true);
    setError("");
    const items: BatchItem[] = [];
    const rows = [
      "input,output,template_id,template_version,verdict,ncc,alpha_mae,unstable_pixels,clamped_pixels,outside_diff,flags",
    ];
    for (const file of Array.from(files)) {
      try {
        const encoded = await fileToBase64(file);
        const response = await processImage({
          imageBase64: encoded,
          templates: payloadTemplates(),
          accept: false,
          jpeg,
          keepExif,
          bbox: bbox ?? undefined,
          cameraTime: cameraPayload(),
        });
        const verdict = VERDICT_LABEL[response.report.verdict];
        items.push({ name: file.name, verdict });
        const ext = response.report.output?.format === "jpeg" ? "jpg" : "png";
        const outName = response.imageBase64 ? `${stripExt(file.name)}.restored.${ext}` : "";
        rows.push(csvRow(file.name, outName, response.report));
        downloadNamed(
          `${stripExt(file.name)}.restored.report.json`,
          new Blob([JSON.stringify(response.report, null, 2)], { type: "application/json" }),
        );
        if (response.imageBase64) {
          const mime = ext === "jpg" ? "image/jpeg" : "image/png";
          downloadNamed(outName, base64ToBlob(response.imageBase64, mime));
        }
        if (file === files[0]) {
          setImageName(file.name);
          setImageBase64(encoded);
          setReport(response.report);
          setProbe(response.probe);
          setResultBase64(response.imageBase64);
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : "处理失败";
        items.push({ name: file.name, verdict: "失败" });
        rows.push(`${csvCell(file.name)},,,failed,,,,,,"${message.replaceAll('"', "'")}"`);
      }
    }
    downloadNamed("summary.csv", new Blob([rows.join("\n") + "\n"], { type: "text/csv" }));
    setBatch(items);
    setBusy(false);
  }

  function readProbe(event: ReactPointerEvent<HTMLDivElement>, canvas: HTMLCanvasElement | null) {
    if (!canvas?.width) return;
    const point = eventPoint(event, canvas);
    if (!point) return;
    if (drag.current && manual) {
      const next = boxFrom(drag.current, point, canvas.width, canvas.height);
      draftRef.current = next;
      setDraft(next);
    }
    const before = pixelAt(canvas, point.x, point.y);
    const after = pixelAt(resultRef.current, point.x, point.y);
    let extra = "";
    if (probe) {
      const [bx, by, bw, bh] = probe.bbox;
      if (point.x >= bx && point.y >= by && point.x < bx + bw && point.y < by + bh) {
        const alpha = base64ToBytes(probe.alphaBase64);
        const unstable = base64ToBytes(probe.unstableBase64);
        const index = (point.y - by) * bw + (point.x - bx);
        const value = alpha[index] ?? 0;
        extra = `，模板 α ${(value / 255).toFixed(3)}，${unstable[index] ? "不稳定" : "稳定"}`;
      }
    }
    const beforeText = before ? `${before[0]} ${before[1]} ${before[2]}` : "—";
    const afterText = holdOriginal ? "显示原图" : after ? `${after[0]} ${after[1]} ${after[2]}` : "—";
    setProbeText(`${point.x}, ${point.y}  处理前 ${beforeText}  处理后 ${afterText}${extra}`);
  }

  function startBox(event: ReactPointerEvent<HTMLDivElement>) {
    if (!manual) return;
    const canvas = originalRef.current;
    const point = canvas ? eventPoint(event, canvas) : null;
    if (!point) return;
    drag.current = point;
    const next: Box = [point.x, point.y, 1, 1];
    draftRef.current = next;
    setDraft(next);
    event.currentTarget.setPointerCapture(event.pointerId);
  }

  function finishBox() {
    const next = draftRef.current;
    if (drag.current && next && next[2] > 2 && next[3] > 2) setBbox(next);
    drag.current = null;
    draftRef.current = null;
    setDraft(null);
  }

  function exportMask() {
    const canvas = originalRef.current;
    const box = bbox;
    if (!canvas?.width || !box) return;
    const mask = document.createElement("canvas");
    mask.width = canvas.width;
    mask.height = canvas.height;
    const ctx = mask.getContext("2d");
    if (!ctx) return;
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, mask.width, mask.height);
    ctx.fillStyle = "#fff";
    ctx.fillRect(box[0], box[1], box[2], box[3]);
    mask.toBlob((blob) => {
      if (blob) downloadNamed(`${stripExt(imageName)}.mask.png`, blob);
    });
  }

  function downloadResult() {
    if (!resultBase64 || !report) return;
    const ext = report.output?.format === "jpeg" ? "jpg" : "png";
    const mime = ext === "jpg" ? "image/jpeg" : "image/png";
    downloadNamed(`${stripExt(imageName)}.restored.${ext}`, base64ToBlob(resultBase64, mime));
  }

  const shown = draft ?? bbox ?? report?.bbox ?? null;

  return (
    <div
      className={dropping ? "app dropping" : "app"}
      onDragOver={(event) => {
        event.preventDefault();
        setDropping(true);
      }}
      onDragLeave={() => setDropping(false)}
      onDrop={(event) => {
        event.preventDefault();
        setDropping(false);
        const file = event.dataTransfer.files[0];
        if (file && file.type.startsWith("image/")) void onImage(file);
      }}
    >
      <header>
        <div>
          <h1>水印去除</h1>
          <p>本地逆混合。图片留在这台机器上。</p>
        </div>
        <p className="note">{note}</p>
      </header>
      <div className="toolbar">
        <button type="button" onClick={() => void loadSample()} disabled={busy}>加载示例</button>
        <label className="file">
          打开图片
          <input type="file" accept="image/png,image/jpeg" onChange={(event) => void onImage(event.target.files?.[0])} />
        </label>
        <label className="file">
          模板 JSON
          <input id="template-json" type="file" accept="application/json,.json" />
        </label>
        <label className="file">
          Alpha PNG
          <input
            id="template-alpha"
            type="file"
            accept="image/png"
            onChange={() => {
              const json = document.querySelector<HTMLInputElement>("#template-json")?.files?.[0];
              const alpha = document.querySelector<HTMLInputElement>("#template-alpha")?.files?.[0];
              const bias = document.querySelector<HTMLInputElement>("#template-bias")?.files?.[0];
              void onTemplate(json, alpha, bias);
            }}
          />
        </label>
        <label className="file">
          Bias PNG
          <input
            id="template-bias"
            type="file"
            accept="image/png"
            onChange={() => {
              const json = document.querySelector<HTMLInputElement>("#template-json")?.files?.[0];
              const alpha = document.querySelector<HTMLInputElement>("#template-alpha")?.files?.[0];
              const bias = document.querySelector<HTMLInputElement>("#template-bias")?.files?.[0];
              if (json && alpha) void onTemplate(json, alpha, bias);
            }}
          />
        </label>
        <label className="file">
          模板目录
          <input ref={dirInput} type="file" multiple onChange={(event) => void onTemplateDir(event.target.files)} />
        </label>
        <button className="primary" type="button" disabled={busy || !imageBase64} onClick={() => ask(() => void runProcess(false))}>
          处理
        </button>
        <button type="button" disabled={busy || !imageBase64} onClick={() => ask(() => void runProcess(true))}>
          确认低置信度
        </button>
        <label className="check">
          <input type="checkbox" checked={jpeg} onChange={(event) => setJpeg(event.target.checked)} />
          JPEG（有损，不能作为像素级结果）
        </label>
        <label className="check">
          <input type="checkbox" checked={keepExif} onChange={(event) => setKeepExif(event.target.checked)} />
          保留方向已校正的 EXIF
        </label>
        <label className="check">
          <input type="checkbox" checked={manual} onChange={(event) => setManual(event.target.checked)} />
          手动框选
        </label>
        <label className="check">
          <input
            type="checkbox"
            checked={editTime}
            onChange={(event) => {
              setEditTime(event.target.checked);
              if (!event.target.checked) setTimeNote("勾选后会识别图中的相机时间，可改年月日时分秒。");
            }}
          />
          修改相机时间
        </label>
        <button type="button" onClick={() => setZoom(1)}>原始大小</button>
        <label className="file">
          批量
          <input
            type="file"
            accept="image/png,image/jpeg"
            multiple
            onChange={(event) => {
              const files = event.target.files;
              if (files && files.length > 0) ask(() => void runBatch(files));
            }}
          />
        </label>
        <button type="button" disabled={!resultBase64} onClick={downloadResult}>下载结果</button>
        <button
          type="button"
          disabled={!report}
          onClick={() => {
            if (!report) return;
            downloadNamed(
              `${stripExt(imageName)}.restored.report.json`,
              new Blob([JSON.stringify(report, null, 2)], { type: "application/json" }),
            );
          }}
        >
          下载报告
        </button>
        <button type="button" disabled={!bbox} onClick={exportMask}>只导出蒙版</button>
      </div>
      {editTime ? (
        <div className="time-row">
          <TimeField label="年" value={year} min={1990} max={2099} className="year" onChange={setYear} />
          <TimeField label="月" value={month} min={1} max={12} onChange={setMonth} />
          <TimeField label="日" value={day} min={1} max={31} onChange={setDay} />
          <TimeField label="时" value={hour} min={0} max={23} onChange={setHour} />
          <TimeField label="分" value={minute} min={0} max={59} onChange={setMinute} />
          <TimeField label="秒" value={second} min={0} max={59} disabled={!hasSecond} onChange={setSecond} />
          <span className="note">{timeNote}</span>
        </div>
      ) : null}
      {error ? <p className="error">{error}</p> : null}
      <main className="workspace">
        <section className="viewers">
          <article className="panel">
            <h2>原图 {Math.round(zoom * 100)}%</h2>
            <div
              className="stage-wrap"
              ref={originalWrap}
              onPointerDown={startBox}
              onPointerMove={(event) => readProbe(event, originalRef.current)}
              onPointerUp={finishBox}
              onPointerCancel={finishBox}
            >
              <div className="stage" style={{ width: `${zoom * 100}%` }}>
                <canvas ref={originalRef} className="view" />
                {shown && size.w > 0 ? <BoxOverlay box={shown} width={size.w} height={size.h} /> : null}
              </div>
            </div>
          </article>
          <article className="panel">
            <h2>{holdOriginal ? "结果（正在显示原图）" : "结果"}</h2>
            <div className="stage-wrap" ref={resultWrap} onPointerMove={(event) => readProbe(event, resultRef.current)}>
              <div className="stage" style={{ width: `${zoom * 100}%` }}>
                <canvas ref={resultRef} className="view" />
              </div>
            </div>
            <p className="probe">{probeText}</p>
          </article>
        </section>
        <aside className="panel">
          <h2>报告</h2>
          {report ? (
            <ReportView report={report} />
          ) : (
            <p className="note">处理后显示模板、置信度和判决。没有干净原图时不计算 PSNR。</p>
          )}
          <div className="batch">
            <h2>批量</h2>
            {batch.length === 0 ? (
              <p className="note">一次处理一张。失败项不会停掉后面的图片，结束时下载 summary.csv。</p>
            ) : (
              <ul>
                {batch.map((item) => (
                  <li key={item.name}>
                    <span>{item.name}</span>
                    <span>{item.verdict}</span>
                  </li>
                ))}
              </ul>
            )}
          </div>
          <p className="note">{COMPLIANCE}</p>
        </aside>
      </main>
      {!accepted ? (
        <div className="modal">
          <div className="card">
            <h2>使用前确认</h2>
            <p>{COMPLIANCE}</p>
            <div className="actions">
              <button className="primary" type="button" onClick={() => setAccepted(true)}>我已理解</button>
            </div>
          </div>
        </div>
      ) : null}
      {confirming ? (
        <div className="modal">
          <div className="card">
            <h2>确认导出</h2>
            <p>{COMPLIANCE}</p>
            <div className="actions">
              <button type="button" onClick={() => { pending.current = null; setConfirming(false); }}>取消</button>
              <button
                className="primary"
                type="button"
                onClick={() => {
                  const action = pending.current;
                  pending.current = null;
                  setConfirming(false);
                  action?.();
                }}
              >
                继续
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}

function ReportView({ report }: { report: Report }) {
  const quality = report.qualityClaim === "pixel"
    ? "像素级（以该模板的留出验证为准）"
    : report.qualityClaim === "visual"
      ? "视觉级"
      : "—";
  const rows: Array<[string, string]> = [
    ["判决", VERDICT_LABEL[report.verdict]],
    ["模板", report.templateId ? `${report.templateId} v${report.templateVersion ?? "?"}` : "—"],
    ["质量声明", quality],
    ["混合 / 空间", [report.blendMode, report.colorSpace].filter(Boolean).join(" / ") || "—"],
    ["NCC", report.ncc === null ? "—" : report.ncc.toFixed(3)],
    ["α 平均误差", report.alphaMae === null ? "—" : report.alphaMae.toFixed(3)],
    ["位移", report.shift ? `${report.shift[0]}, ${report.shift[1]}` : "—"],
    ["核心像素", String(report.counts.corePixels)],
    ["不稳定像素", String(report.counts.unstablePixels)],
    ["夹紧像素", String(report.counts.clampedPixels)],
    ["框外差异", String(report.counts.outsideDiffPixels)],
  ];
  if (report.cameraTime) {
    rows.push(["相机时间", `${report.cameraTime.original} → ${report.cameraTime.updated}`]);
  }
  return (
    <div>
      <span className={`verdict ${report.verdict}`}>{VERDICT_LABEL[report.verdict]}</span>
      <dl>
        {rows.map(([key, value]) => (
          <span key={key} style={{ display: "contents" }}>
            <dt>{key}</dt>
            <dd>{value}</dd>
          </span>
        ))}
      </dl>
      {report.flags.includes("lossy_output") ? <p className="note">JPEG 有损，这张结果不能作为像素级还原。</p> : null}
      {report.cameraTime ? <p className="note">相机时间已改写，字号和粗细按原水印匹配。机型那一行没有改。</p> : null}
      {report.flags.includes("camera_time_missing") ? <p className="note">没有找到可改的相机时间，日期保持原样。</p> : null}
      {report.flags.includes("camera_time_invalid") ? <p className="note">日期或时间不合法，没有改相机时间。</p> : null}
      {report.flags.includes("camera_time_skipped") ? <p className="note">这张图还没写出结果，相机时间也没有改。</p> : null}
      {report.flags.length > 0 ? (
        <div className="flags">{report.flags.map((flag) => <span key={flag}>{flag}</span>)}</div>
      ) : null}
    </div>
  );
}

function TimeField({
  label,
  value,
  min,
  max,
  disabled,
  className,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  disabled?: boolean;
  className?: string;
  onChange: (value: number) => void;
}) {
  return (
    <label>
      {label}
      <input
        className={className}
        type="number"
        min={min}
        max={max}
        disabled={disabled}
        value={value}
        onChange={(event) => {
          const next = Number(event.target.value);
          if (Number.isFinite(next)) onChange(Math.trunc(next));
        }}
      />
    </label>
  );
}

function BoxOverlay({ box, width, height }: { box: Box; width: number; height: number }) {
  const [x, y, w, h] = box;
  return (
    <div
      className="rect"
      style={{
        left: `${(x / width) * 100}%`,
        top: `${(y / height) * 100}%`,
        width: `${(w / width) * 100}%`,
        height: `${(h / height) * 100}%`,
      }}
    />
  );
}

function useZoom(ref: { current: HTMLDivElement | null }, setZoom: (value: number | ((prev: number) => number)) => void) {
  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const factor = event.deltaY > 0 ? 0.92 : 1.08;
      setZoom((prev) => Math.min(6, Math.max(0.4, prev * factor)));
    };
    node.addEventListener("wheel", onWheel, { passive: false });
    return () => node.removeEventListener("wheel", onWheel);
  }, [ref, setZoom]);
}

function eventPoint(event: ReactPointerEvent<HTMLDivElement>, canvas: HTMLCanvasElement) {
  const rect = canvas.getBoundingClientRect();
  if (!rect.width || !rect.height || !canvas.width || !canvas.height) return null;
  const x = clamp(Math.floor(((event.clientX - rect.left) / rect.width) * canvas.width), 0, canvas.width - 1);
  const y = clamp(Math.floor(((event.clientY - rect.top) / rect.height) * canvas.height), 0, canvas.height - 1);
  return { x, y };
}

function boxFrom(start: { x: number; y: number }, end: { x: number; y: number }, width: number, height: number): Box {
  const x = clamp(Math.min(start.x, end.x), 0, width - 1);
  const y = clamp(Math.min(start.y, end.y), 0, height - 1);
  const w = clamp(Math.abs(end.x - start.x), 1, width - x);
  const h = clamp(Math.abs(end.y - start.y), 1, height - y);
  return [x, y, w, h];
}

function pixelAt(canvas: HTMLCanvasElement | null, x: number, y: number): [number, number, number] | null {
  if (!canvas || x < 0 || y < 0 || x >= canvas.width || y >= canvas.height) return null;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) return null;
  const pixel = ctx.getImageData(x, y, 1, 1).data;
  return [pixel[0], pixel[1], pixel[2]];
}

function drawImage(canvas: HTMLCanvasElement | null, image: HTMLImageElement) {
  if (!canvas) return;
  canvas.width = image.naturalWidth;
  canvas.height = image.naturalHeight;
  canvas.getContext("2d")?.drawImage(image, 0, 0);
}

function clearCanvas(canvas: HTMLCanvasElement | null) {
  if (!canvas) return;
  canvas.width = 0;
  canvas.height = 0;
}

function stripExt(name: string): string {
  return name.replace(/\.[^.]+$/, "");
}

function csvCell(value: string): string {
  return /[",\n]/.test(value) ? `"${value.replaceAll('"', '""')}"` : value;
}

function csvRow(name: string, output: string, report: Report): string {
  return [
    name,
    output,
    report.templateId ?? "",
    report.templateVersion === null ? "" : String(report.templateVersion),
    report.verdict,
    report.ncc === null ? "" : report.ncc.toFixed(4),
    report.alphaMae === null ? "" : report.alphaMae.toFixed(4),
    String(report.counts.unstablePixels),
    String(report.counts.clampedPixels),
    String(report.counts.outsideDiffPixels),
    report.flags.join("|"),
  ].map(csvCell).join(",");
}

function downloadNamed(name: string, blob: Blob) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}
