package basalt.oracle.mixin;

import basalt.oracle.Anim;
import com.mojang.blaze3d.vertex.PoseStack;
import net.minecraft.client.renderer.GameRenderer;
import net.minecraft.client.renderer.state.level.CameraRenderState;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Test-only: records the view-bobbing transform (Anim "fp" recorder) and frame boundaries. */
@Mixin(GameRenderer.class)
public abstract class BobMixin {
    @Inject(method = "bobView", at = @At("TAIL"))
    private void basalt$bob(CameraRenderState cam, PoseStack ps, CallbackInfo ci) {
        Anim.fpRecord("bob", ps, null);
    }

    @Inject(method = "render", at = @At("TAIL"))
    private void basalt$frame(net.minecraft.client.DeltaTracker dt, boolean b, CallbackInfo ci) {
        Anim.frameDone();
    }
}
