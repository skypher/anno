// Ghidra script to export decompiled C code
// @category Export
// @author Anno1602 RE

import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import java.io.File;
import java.io.FileWriter;
import java.io.PrintWriter;

public class ExportDecomp extends GhidraScript {
    @Override
    public void run() throws Exception {
        DecompInterface decomp = new DecompInterface();
        decomp.openProgram(currentProgram);

        String homeDir = System.getProperty("user.home");
        File outputDir = new File(homeDir + "/anno/decompiled");
        outputDir.mkdirs();

        String progName = currentProgram.getName().replace(".", "_");
        File outputFile = new File(outputDir, progName + ".c");

        PrintWriter writer = new PrintWriter(new FileWriter(outputFile));
        writer.println("// Decompiled from: " + currentProgram.getName());
        writer.println();

        FunctionIterator funcIter = currentProgram.getFunctionManager().getFunctions(true);
        int count = 0;
        while (funcIter.hasNext() && !monitor.isCancelled()) {
            Function func = funcIter.next();
            DecompileResults results = decomp.decompileFunction(func, 30, monitor);
            if (results != null && results.decompileCompleted()) {
                if (results.getDecompiledFunction() != null) {
                    String body = results.getDecompiledFunction().getC();
                    writer.println("// Function at " + func.getEntryPoint().toString());
                    writer.println("// Name: " + func.getName());
                    writer.println(body);
                    writer.println();
                    count++;
                }
            }
        }
        writer.close();
        decomp.dispose();
        println("Exported " + count + " functions to " + outputFile.getAbsolutePath());
    }
}
