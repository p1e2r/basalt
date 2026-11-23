package basalt.oracle.mixin;

import basalt.oracle.Anim;
import com.mojang.blaze3d.vertex.PoseStack;
import net.minecraft.client.renderer.SubmitNodeCollector;
import net.minecraft.client.renderer.entity.player.AvatarRenderer;
import net.minecraft.resources.Identifier;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/**
 * Test-only: records the pose at which the first-person (empty) right arm is drawn (Anim "fp"
 * recorder), and afterwards the arm model part's pose as it was drawn.
 */
@Mixin(AvatarRenderer.class)
public abstract class ArmMixin {
    @Inject(method = "renderRightHand", at = @At("HEAD"))
    private void basalt$arm(PoseStack ps, SubmitNodeCollector c, int light, Identifier skin, boolean sleeve, CallbackInfo ci) {
        Anim.fpRecord("arm", ps, null);
    }

    @Inject(method = "renderRightHand", at = @At("TAIL"))
    private void basalt$armPart(PoseStack ps, SubmitNodeCollector c, int light, Identifier skin, boolean sleeve, CallbackInfo ci) {
        var p = ((AvatarRenderer<?>) (Object) this).getModel().rightArm;
        Anim.fpPart("arm_part", new float[] { p.x, p.y, p.z, p.xRot, p.yRot, p.zRot });
    }
}
