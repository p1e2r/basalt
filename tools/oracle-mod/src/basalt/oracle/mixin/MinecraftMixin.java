package basalt.oracle.mixin;

import basalt.oracle.Control;
import net.minecraft.client.Minecraft;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Test-only: runs the oracle control channel's queued commands at the start of every client tick. */
@Mixin(Minecraft.class)
public abstract class MinecraftMixin {
    @Inject(method = "tick", at = @At("HEAD"))
    private void basalt$tick(CallbackInfo ci) {
        Control.onTick((Minecraft) (Object) this);
    }
}
