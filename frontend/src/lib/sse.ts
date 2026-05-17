export async function readEventStream(
  response: Response,
  onEvent: (event: string, data: unknown) => void,
) {
  if (!response.body) {
    throw new Error("Streaming is not available in this browser");
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";

  while (true) {
    const { value, done } = await reader.read();
    if (done) break;

    buffer += decoder.decode(value, { stream: true });
    const frames = buffer.split("\n\n");
    buffer = frames.pop() ?? "";

    for (const frame of frames) {
      const parsed = parseFrame(frame);
      if (parsed) onEvent(parsed.event, parsed.data);
    }
  }

  buffer += decoder.decode();
  const parsed = parseFrame(buffer);
  if (parsed) onEvent(parsed.event, parsed.data);
}

function parseFrame(frame: string) {
  if (!frame.trim()) return null;

  let event = "message";
  const dataLines: string[] = [];

  for (const line of frame.split(/\r?\n/)) {
    if (line.startsWith("event:")) {
      event = line.slice("event:".length).trim();
    } else if (line.startsWith("data:")) {
      dataLines.push(line.slice("data:".length).trimStart());
    }
  }

  if (dataLines.length === 0) return null;

  const raw = dataLines.join("\n");
  try {
    return { event, data: JSON.parse(raw) as unknown };
  } catch {
    return { event, data: raw };
  }
}
