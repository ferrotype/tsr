/** @param {...number} ns */
function sum(...ns) {
  return ns.length;
}
sum(1, 2);
sum(1, "two");
/** @param {number[]} ms */
function total(...ms) {
  return ms.length;
}
total(1, "two");
