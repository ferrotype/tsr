/**
 * @overload
 * @param {string} x
 * @returns {string}
 *
 * @overload
 * @param {number} x
 * @returns {number}
 *
 * @param {string | number} x
 * @returns {string | number}
 */
let f = x => x;
/**
 * @type {{
 *   (x: string): string;
 *   (x: number): number;
 * }}
 * @param {string | number} x
 * @returns {any}
 */
const g = x => x;
const s = g("s");
const n = g(1);
const bad = g(true);
