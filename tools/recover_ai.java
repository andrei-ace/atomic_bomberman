// Development only. Run with Ghidra analyzeHeadless against an immutable copy.
// Explicit entry points matter: the callback table is an indirect call site.
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import java.io.*;

public class recover_ai extends GhidraScript {
    @Override public void run() throws Exception {
        String[] argv = getScriptArgs();
        File out = new File(argv[0]); out.mkdirs();
        long[] callbacks = {0x40bd44L,0x40be02L,0x40b20fL,0x40ad8dL,
            0x40abedL,0x40baf5L,0x40b8c2L,0x40a81fL};
        for (long entry: callbacks) {
            var a = toAddr(entry);
            if (getFunctionAt(a) == null) {
                disassemble(a);
                createFunction(a, "ai_callback_" + Long.toHexString(entry));
            }
        }
        analyzeChanges(currentProgram);
        DecompInterface d = new DecompInterface();
        d.openProgram(currentProgram);
        // Include direct helpers, two levels deep, plus dispatcher and C RNG.
        java.util.Set<Function> work = new java.util.LinkedHashSet<>();
        for (long entry: callbacks) {
            var f = getFunctionAt(toAddr(entry));
            if(f != null) work.add(f);
        }
        var dispatch = getFunctionAt(toAddr(0x40a1c6L));
        if(dispatch != null) work.add(dispatch);
        for(int depth=0;depth<2;depth++) {
            var next = new java.util.ArrayList<>(work);
            for(var f:next) work.addAll(f.getCalledFunctions(monitor));
        }
        for (var f : work) {
            if(f.isExternal()) continue;
            String id=f.getEntryPoint().toString();
            try (PrintWriter p = new PrintWriter(new File(out, id + ".c"))) {
                var result=d.decompileFunction(f,60,monitor);
                if(result.decompileCompleted()) p.print(result.getDecompiledFunction().getC());
                else p.print("// Decompilation failed: " + result.getErrorMessage());
            }
            try(PrintWriter p=new PrintWriter(new File(out,id+".asm"))) {
                var ins=currentProgram.getListing().getInstructions(f.getBody(),true);
                while(ins.hasNext()) {var i=ins.next();p.println(i.getAddress()+": "+i.toString());}
            }
            println("RECOVERED " + id + " " + f.getBody().getNumAddresses());
        }
        d.dispose();
    }
}
