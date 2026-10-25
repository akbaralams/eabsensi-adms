"""Membangkitkan fixture handshake dari kode PHP lama (`eabsensi-99`), apa adanya.

Skrip membaca pernyataan `$rsp = ...;` / `$rsp .= ...;` pada rentang baris
`IDeviceController::iconnect`, mengevaluasi rangkaian string literal dan variabelnya, lalu menulis
byte hasilnya ke `tests/fixtures/handshake/`. Uji Rust membandingkan keluaran `protocol::handshake`
dengan berkas-berkas itu, jadi kecocokan "byte-per-byte dengan acuan lama" terbukti terhadap
sumbernya, bukan terhadap ingatan atau salinan manual.

Jalankan ulang bila acuan lama berubah (tidak seharusnya):

    python tests/fixtures/generate_handshake.py [path/ke/IDeviceController.php]
"""

import pathlib
import re
import sys

DEFAULT_SOURCE = pathlib.Path(__file__).resolve().parents[3] / "eabsensi-99/app/Http/Controllers/IDeviceController.php"
OUT_DIR = pathlib.Path(__file__).resolve().parent / "handshake"

# (nama, baris awal, baris akhir, potongan teks yang harus ada di baris awal)
RANGES = [
    ("basic", 89, 100, 'GET OPTION FROM:'),  # mesin baru / firmware lama (TransFlag=111111111111)
    ("att", 104, 120, 'GET OPTION FROM:'),  # DeviceType att (menimpa $rsp)
    ("push23", 183, 197, 'GET OPTION FROM:'),  # firmware dengan push_version tercatat
]

SERIAL = "SN-CONTOH-0001"
SCENARIOS = {
    "stamps": {"tflag": "111", "oflag": "222", "pflag": "333", "bioflag": "444"},
    "none": {"tflag": "None", "oflag": "None", "pflag": "None", "bioflag": "None"},
}
COMMON = {"sn": SERIAL, "dev->err_delay": "60", "dev->delay": "30", "dev->timezone": "7"}

TOKEN = re.compile(r'"(?:[^"\\]|\\.)*"|\$[A-Za-z_]+(?:->[A-Za-z_]+)?')
STATEMENT = re.compile(r'^\s*\$rsp\s*(\.?=)\s*(.+?);\s*$')


def php_string(literal: str) -> str:
    body = literal[1:-1]
    return (
        body.replace("\\n", "\n").replace("\\t", "\t").replace('\\"', '"').replace("\\\\", "\\")
    )


def evaluate(expr: str, values: dict) -> str:
    out = []
    for token in TOKEN.findall(expr):
        if token.startswith('"'):
            out.append(php_string(token))
        else:
            name = token[1:]
            if name not in values:
                raise SystemExit(f"variabel PHP tidak dikenal: {token} di '{expr}'")
            out.append(values[name])
    return "".join(out)


def build(lines: list[str], start: int, end: int, marker: str, values: dict) -> str:
    if marker not in lines[start - 1]:
        raise SystemExit(f"baris {start} bukan awal blok yang diharapkan: {lines[start - 1]!r}")
    rsp = ""
    for number in range(start, end + 1):
        match = STATEMENT.match(lines[number - 1])
        if not match:
            raise SystemExit(f"baris {number} bukan pernyataan $rsp: {lines[number - 1]!r}")
        value = evaluate(match.group(2), values)
        rsp = value if match.group(1) == "=" else rsp + value
    return rsp


def main() -> None:
    source = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_SOURCE
    lines = source.read_text(encoding="utf-8").splitlines()
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    for scenario, stamps in SCENARIOS.items():
        values = {**COMMON, **stamps}
        for name, start, end, marker in RANGES:
            text = build(lines, start, end, marker, values)
            target = OUT_DIR / f"{name}_{scenario}.txt"
            target.write_bytes(text.encode("utf-8"))
            print(f"{target.name}: {len(text)} byte")


if __name__ == "__main__":
    main()
