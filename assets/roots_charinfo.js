function charinfo(s) {
  let r = [];

  for (let c of s) {
    let i = 0;
    let j = c.codePointAt(0);

    while (i < chars.length) {
      if (j >= chars[i] && j < chars[i] + chars[i + 1].length) {
        j -= chars[i];

        let a = { char: c, info: null };
        let b = chars[i + 1][j];

        if (! b) {
          r.push(a);
          break;
        }

        let info = { codes: Array.isArray(b[0]) ? b[0] : [ b[0] ], chaifens: [] };
        a.info = info;

        if (Array.isArray(b[1])) {
          for (let k = 1; k < b.length; ++k) {
            let chaifen = [];

            for (let l = 0; l < b[k].length; ++l) {
              chaifen.push({ root: roots[2 * b[k][l]], code: roots[2 * b[k][l] + 1] });
            }

            info.chaifens.push(chaifen);
          }
        } else {
          let chaifen = [];

          for (let k = 1; k < b.length; ++k) {
            chaifen.push({ root: roots[2 * b[k]], code: roots[2 * b[k] + 1] });
          }

          info.chaifens.push(chaifen);
        }

        r.push(a);
        break;
      }

      i += 2;
    }

    if (i >= chars.length) r.push([c, null]);
  }

  return r;
}
