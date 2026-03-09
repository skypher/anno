# Ghidra headless script to export decompiled C code
# @category: Export

from ghidra.app.decompiler import DecompInterface
from ghidra.util.task import ConsoleTaskMonitor
import os

decomp = DecompInterface()
decomp.openProgram(currentProgram)

monitor = ConsoleTaskMonitor()

output_dir = os.path.join(os.path.expanduser("~"), "anno", "decompiled")
if not os.path.exists(output_dir):
    os.makedirs(output_dir)

prog_name = currentProgram.getName().replace(".", "_")
output_file = os.path.join(output_dir, prog_name + ".c")

listing = currentProgram.getListing()
func_manager = currentProgram.getFunctionManager()

with open(output_file, "w") as f:
    f.write("// Decompiled from: %s\n\n" % currentProgram.getName())

    # Export all functions
    func_iter = func_manager.getFunctions(True)
    count = 0
    for func in func_iter:
        if monitor.isCancelled():
            break
        results = decomp.decompileFunction(func, 30, monitor)
        if results and results.decompileCompleted():
            decomp_func = results.getDecompiledFunction()
            if decomp_func:
                sig = decomp_func.getSignature()
                body = decomp_func.getC()
                f.write("// Function at 0x%s\n" % func.getEntryPoint().toString())
                f.write(body)
                f.write("\n\n")
                count += 1

    print("Exported %d functions to %s" % (count, output_file))

decomp.dispose()
