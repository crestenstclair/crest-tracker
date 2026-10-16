/* Text editing boundary. Rust retains musical range/capability validation. */
const CrestTrackerEditValues = (() => {
  "use strict";
  const { parseBeat, beatLabel, parseTempoMilliBpm } = CrestTrackerTimeValues;

  function parseSafeDecimalInteger(text) {
    const raw = String(text ?? "").trim(), number = Number(raw);
    if (!/^[+-]?\d+$/.test(raw) || !Number.isSafeInteger(number)) throw Error("FX values must be exact integers");
    return number;
  }

  // Omitted expectedCount permits any nonempty list of intervals.
  function parseNonemptyCommaFields(text, expectedCount) {
    const parts = String(text ?? "").split(",").map(part => part.trim());
    if (parts.some(part => !part) || (expectedCount !== undefined && parts.length !== expectedCount)) {
      throw Error(expectedCount === undefined ? "Enter comma-separated intervals" : `Enter exactly ${expectedCount} nonempty value${expectedCount === 1 ? "" : "s"}`);
    }
    return parts;
  }

  function parseBoolean(text) {
    if (!["true", "false"].includes(text)) throw Error("Open voicing must be true or false");
    return text === "true";
  }

  const field = (name, parse = parseSafeDecimalInteger, format = String) => ({ name, parse, format });
  const timeField = name => field(name, parseBeat, beatLabel);
  function tupleCodec(fields) {
    return {
      parse(text) {
        const parts = parseNonemptyCommaFields(text, fields.length);
        return Object.fromEntries(fields.map((field, index) => [field.name, field.parse(parts[index])]));
      },
      format(value) { return fields.map(field => field.format(value[field.name])).join(","); }
    };
  }
  const intervals = {
    parse: text => ({ intervals: parseNonemptyCommaFields(text).map(parseSafeDecimalInteger) }),
    format: value => value.intervals.join(",")
  };
  // One ordered field definition owns both directions, independent of JSON order.
  const codecs = new Map(Object.entries({
    offset: tupleCodec([field("semitones")]), velocity: tupleCodec([field("value")]),
    gate: tupleCodec([timeField("fraction")]), cut: tupleCodec([timeField("after")]),
    delay: tupleCodec([timeField("beats")]), retrigger: tupleCodec([field("count"), timeField("spacing")]),
    chance: tupleCodec([field("percent")]), cc: tupleCodec([field("number"), field("value")]),
    poly_pressure: tupleCodec([field("pitch"), field("value")]), bend: tupleCodec([field("value")]),
    pressure: tupleCodec([field("value")]), program: tupleCodec([field("value")]),
    tempo: tupleCodec([field("tempo", parseTempoMilliBpm, value => String(value / 1000))]),
    jump: tupleCodec([field("step", text => parseSafeDecimalInteger(text) - 1, value => String(value + 1))]),
    chord: intervals, add: intervals, voicing: tupleCodec([field("inversion"), field("open", parseBoolean)]),
    spread: tupleCodec([field("voices")]), send: tupleCodec([field("bus"), field("amount"), timeField("duration")])
  }));
  const controls = new Set(["cc", "poly_pressure", "bend", "pressure", "program"]);
  function codecFor(kind) {
    const codec = codecs.get(kind);
    if (!codec) throw Error("Unknown FX");
    return codec;
  }
  function parseFx(kind, text) {
    if (kind === "none") return null;
    if (kind.startsWith("bank:")) {
      const revision = CrestTrackerWireValues.unsignedInteger(kind.slice(5));
      if (revision < 1) throw Error("Choose a saved chord revision");
      return { kind: "chord_revision", revision };
    }
    const parsed = { kind, ...codecFor(kind).parse(text) };
    return controls.has(kind) ? { kind: "control", control: parsed } : parsed;
  }
  function formatFx(fx) {
    if (!fx) return "";
    const value = fx.kind === "control" ? fx.control : fx;
    return codecFor(value.kind).format(value);
  }

  function maximumExpressionSequence(points) {
    return points.reduce((maximum, point) => {
      const sequence = BigInt(CrestTrackerWireValues.unsignedInteger(point.sequence ?? 0));
      return sequence > maximum ? sequence : maximum;
    }, 0n);
  }

  // Member/master points share source ordering. Avoid Number rounding and
  // argument-count limits when adding to long captured expression curves.
  function nextExpressionSequence(note) {
    const member = maximumExpressionSequence(note.expression ?? []);
    const master = maximumExpressionSequence(note.master_expression ?? []);
    const next = (member > master ? member : master) + 1n;
    // Composition's MAX_EXPRESSION_SEQUENCE remains the exact JSON range.
    // Report exhaustion in the open form instead of submitting an invalid edit.
    if (next > BigInt(Number.MAX_SAFE_INTEGER)) throw Error("Expression sequence exhausted");
    return Number(next);
  }

  return Object.freeze({ ...CrestTrackerTimeValues, parseFx, formatFx, nextExpressionSequence });
})();
