/* Exact text timing and unit conversion for the native editing boundary. */
const CrestTrackerTimeValues = (() => {
  "use strict";
  const { unsignedInteger } = CrestTrackerWireValues;

  function reducedBeat(numerator, denominator) {
    if (numerator < 0n || denominator <= 0n) throw Error("Time must be nonnegative with a positive denominator");
    let left = numerator, right = denominator;
    while (right) [left, right] = [right, left % right];
    numerator /= left;
    denominator /= left;
    return { numerator: unsignedInteger(numerator), denominator: unsignedInteger(denominator) };
  }

  function parseBeat(text) {
    const raw = String(text ?? "").trim();
    const fraction = /^\+?(\d+)\s*\/\s*(\d+)$/.exec(raw);
    if (fraction) return reducedBeat(BigInt(fraction[1]), BigInt(fraction[2]));
    if (!/^\+?(?:\d+(?:\.\d*)?|\.\d+)$/.test(raw)) {
      throw Error("Time must be a nonnegative decimal beat value or exact fraction");
    }
    const [whole, decimal = ""] = raw.replace(/^\+/, "").split(".");
    const digits = decimal.replace(/0+$/, "");
    return reducedBeat(BigInt((whole || "0") + digits), 10n ** BigInt(digits.length));
  }

  function beatLabel(position) {
    const numerator = BigInt(position.numerator), denominator = BigInt(position.denominator);
    return numerator % denominator === 0n
      ? String(numerator / denominator)
      : position.numerator + "/" + position.denominator;
  }

  function scaleBeat(position, numerator, denominator = 1) {
    return reducedBeat(BigInt(position.numerator) * BigInt(numerator), BigInt(position.denominator) * BigInt(denominator));
  }

  // BPM text becomes the positive integer milli-BPM wire value.
  function parseTempoMilliBpm(text) {
    const bpm = parseBeat(text);
    const scaled = BigInt(bpm.numerator) * 1000n, denominator = BigInt(bpm.denominator);
    if (scaled % denominator) {
      throw Error("Tempo must be exact to 0.001 BPM");
    }
    if (scaled === 0n || scaled / denominator > 4294967295n) {
      throw Error("Tempo must be from 0.001 to 4294967.295 BPM");
    }
    return Number(scaled / denominator);
  }

  function compareBeats(left, right) {
    const difference = BigInt(left.numerator) * BigInt(right.denominator)
      - BigInt(right.numerator) * BigInt(left.denominator);
    return difference < 0n ? -1 : difference > 0n ? 1 : 0;
  }

  return Object.freeze({ parseBeat, beatLabel, scaleBeat, parseTempoMilliBpm, compareBeats });
})();
