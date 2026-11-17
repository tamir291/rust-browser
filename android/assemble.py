"""Add the compiled DEX payload to the Android resource package."""

from pathlib import Path
from shutil import copyfile
from sys import argv
from zipfile import ZIP_DEFLATED, ZipFile

source, dex, result = map(Path, argv[1:])
copyfile(source, result)
with ZipFile(result, "a", compression=ZIP_DEFLATED) as archive:
    archive.write(dex, "classes.dex")
