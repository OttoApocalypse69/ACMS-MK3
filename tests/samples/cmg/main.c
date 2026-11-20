#include "raylib.h"
#include <stdio.h>
#include <string.h>
#include <stdbool.h>
// #include <stdlib.h>

#define WINDOW_W 800
#define WINDOW_H 600

int main(int argc, char *argv[])
{
	char main[10240] = "";
	int count = 0; 	
	int ch;
    if (argc > 1)
    {
		printf("[INFO] arg: %s\n", argv[1] );

		FILE *fp =  fopen(argv[1], "r");

		 if (fp == NULL) {
       		perror("[ERROR] Error opening file");	 
		return 1;
   		 }	
		int indx = 0;
		printf("[INFO] Img size: %d\n", (int)sizeof(main) );
  	  	while ((ch = fgetc(fp)) != EOF && indx < (int)sizeof(main) - 1) {
			main[indx] = (char)ch;
			indx++;
        		putchar(ch);
		
		}
		main[indx] = '\0';

		printf("[INFO] Img size: %d\n", (int)sizeof(main) );
  	 	 fclose(fp);
		printf("[INFO] %s read successfuly.\n", argv[1]);
/*		char *token = strtok(argv[1], "\n");
   		 while (token != NULL && count < 100) {
        		size_t len = strlen(token);
       			 if (len > 0 && token[len - 1] == '\r') {
          			  token[len - 1] = '\0';
        		}
			
       		 	lines[count++] = token;
        		token = strtok(NULL, "\n"); 
   		 }    
*/
}
    else
    {
        printf("[ERROR] No args!\n");
	return 5;
    }
 
	int x = 0;
	int y = 0;
	int xtra = 0;
	int ytra = 0;
	
	bool first = true; 	
	int scale = 20;
	int option = 1;
	printf("[OPTION] Please select an option:\n 1 = save the image with 20px cells\n 2 = save the image with 1px cells\n 3 = save the image with 5px cells\n 4 = don't save\n");
	scanf("%d",&option);
	if (option == 2) {
	scale = 1;
	}
	else if (option == 3){
	scale = 5;
	}
	else if (option == 4) {
	first = false;
	}
	else {
	scale = 20;
	}
   InitWindow(WINDOW_W, WINDOW_H, "CMG");
	printf("[INFO] Window initialized\n");
    while (!WindowShouldClose())
    {
	if (first) {

	printf("[INFO] First pass\n");
	BeginDrawing();
		printf("[INFO] Drawing began\n");
            ClearBackground(RAYWHITE);
	printf("[INFO] Background cleared\n");
 DrawText("PREPARING/SAVING TEXTURE AS PNG... \nPlease stay patient!", 20, 20, 20, BLACK);

	EndDrawing();
	printf("[INFO] Drawing ended]n");
	int pwidth = 0;
	int pheight = 0;
	int tempwidth = 0;
	printf("[INFO] Calculating dimensions\n");
	bool firstPass = true;
	
	for (int i = 0; i < strlen(main); i++){
	char c  = main[i];
	
	if (c != '\n') {	
	
		tempwidth += scale;
		printf("[INFO] tempwidth: %d\n", tempwidth);
	}
	else {
		pheight += scale;
		printf("[INFO] height: %d\n", pheight);
		if (tempwidth > pwidth) {
		pwidth = tempwidth;
		printf("[INFO] width: %d\n", pwidth);
		}
		tempwidth = 0;
	}

	}
		if (tempwidth > pwidth) {
		pwidth = tempwidth;
		printf("[INFO] width: %d\n", pwidth);
		}
	printf("[INFO] Second pass\n");
	 RenderTexture2D target = LoadRenderTexture(pwidth, pheight);
	 BeginTextureMode(target);
            ClearBackground(RAYWHITE);
	printf("[INFO] Background cleared\n");
		x = 0 ;
		y = 0 ;
	for (int i = 0; i < strlen(main); i++){
	char c  = main[i];
	
		if (c == '`') { 
   DrawRectangle(x, y, scale, scale, (Color){177, 10, 235,255}  );
} 
else if (c == '1') { 
   DrawRectangle(x, y, scale, scale, (Color){164, 248, 225,255}  );
} 
else if (c == '2') { 
   DrawRectangle(x, y, scale, scale, (Color){187, 75, 75,255}  );
} 
else if (c == '3') { 
   DrawRectangle(x, y, scale, scale, (Color){11, 123, 90,255}  );
} 
else if (c == '4') { 
   DrawRectangle(x, y, scale, scale, (Color){202, 119, 54,255}  );
} 
else if (c == '5') { 
   DrawRectangle(x, y, scale, scale, (Color){89, 35, 87,255}  );
} 
else if (c == '6') { 
   DrawRectangle(x, y, scale, scale, (Color){163, 101, 75,255}  );
} 
else if (c == '7') { 
   DrawRectangle(x, y, scale, scale, (Color){56, 172, 190,255}  );
} 
else if (c == '8') { 
   DrawRectangle(x, y, scale, scale, (Color){68, 145, 148,255}  );
} 
else if (c == '9') { 
   DrawRectangle(x, y, scale, scale, (Color){135, 253, 185,255}  );
} 
else if (c == '0') { 
   DrawRectangle(x, y, scale, scale, (Color){88, 33, 16,255}  );
} 
else if (c == '-') { 
   DrawRectangle(x, y, scale, scale, (Color){69, 100, 246,255}  );
} 
else if (c == '=') { 
   DrawRectangle(x, y, scale, scale, (Color){108, 133, 133,255}  );
} 
else if (c == 'q') { 
   DrawRectangle(x, y, scale, scale, (Color){1, 43, 53,255}  );
} 
else if (c == 'w') { 
   DrawRectangle(x, y, scale, scale, (Color){12, 223, 105,255}  );
} 
else if (c == 'e') { 
   DrawRectangle(x, y, scale, scale, (Color){177, 203, 157,255}  );
} 
else if (c == 'r') { 
   DrawRectangle(x, y, scale, scale, (Color){49, 57, 244,255}  );
} 
else if (c == 't') { 
   DrawRectangle(x, y, scale, scale, (Color){200, 253, 223,255}  );
} 
else if (c == 'y') { 
   DrawRectangle(x, y, scale, scale, (Color){88, 113, 153,255}  );
} 
else if (c == 'u') { 
   DrawRectangle(x, y, scale, scale, (Color){190, 94, 238,255}  );
} 
else if (c == 'i') { 
   DrawRectangle(x, y, scale, scale, (Color){126, 42, 110,255}  );
} 
else if (c == 'o') { 
   DrawRectangle(x, y, scale, scale, (Color){128, 42, 236,255}  );
} 
else if (c == 'p') { 
   DrawRectangle(x, y, scale, scale, (Color){87, 184, 127,255}  );
} 
else if (c == '[') { 
   DrawRectangle(x, y, scale, scale, (Color){253, 229, 208,255}  );
} 
else if (c == ']') { 
   DrawRectangle(x, y, scale, scale, (Color){29, 166, 147,255}  );
} 
else if (c == '\\') { 
   DrawRectangle(x, y, scale, scale, (Color){224, 138, 173,255}  );
} 
else if (c == 'a') { 
   DrawRectangle(x, y, scale, scale, (Color){96, 11, 242,255}  );
}
else if (c == 's') { 
   DrawRectangle(x, y, scale, scale, (Color){109, 94, 17,255}  );
}  
else if (c == 'd') { 
   DrawRectangle(x, y, scale, scale, (Color){71, 3, 153,255}  );
} 
else if (c == 'f') { 
   DrawRectangle(x, y, scale, scale, (Color){5, 231, 214,255}  );
} 
else if (c == 'g') { 
   DrawRectangle(x, y, scale, scale, (Color){7, 27, 126,255}  );
} 
else if (c == 'h') { 
   DrawRectangle(x, y, scale, scale, (Color){157, 138, 125,255}  );
} 
else if (c == 'j') { 
   DrawRectangle(x, y, scale, scale, (Color){73, 250, 185,255}  );
} 
else if (c == 'k') { 
   DrawRectangle(x, y, scale, scale, (Color){248, 95, 102,255}  );
} 
else if (c == 'l') { 
   DrawRectangle(x, y, scale, scale, (Color){169, 157, 32,255}  );
} 
else if (c == ';') { 
   DrawRectangle(x, y, scale, scale, (Color){69, 116, 70,255}  );
} 
else if (c == '\'') { 
   DrawRectangle(x, y, scale, scale, (Color){148, 12, 16,255}  );
} 
else if (c == 'z') { 
   DrawRectangle(x, y, scale, scale, (Color){235, 109, 56,255}  );
} 
else if (c == 'x') { 
   DrawRectangle(x, y, scale, scale, (Color){238, 227, 22,255}  );
} 
else if (c == 'c') { 
   DrawRectangle(x, y, scale, scale, (Color){204, 128, 174,255}  );
} 
else if (c == 'v') { 
   DrawRectangle(x, y, scale, scale, (Color){239, 50, 107,255}  );
} 
else if (c == 'b') { 
   DrawRectangle(x, y, scale, scale, (Color){40, 62, 156,255}  );
} 
else if (c == 'n') { 
   DrawRectangle(x, y, scale, scale, (Color){226, 93, 134,255}  );
} 
else if (c == 'm') { 
   DrawRectangle(x, y, scale, scale, (Color){146, 27, 58,255}  );
} 
else if (c == ',') { 
   DrawRectangle(x, y, scale, scale, (Color){164, 132, 217,255}  );
} 
else if (c == '.') { 
   DrawRectangle(x, y, scale, scale, (Color){185, 42, 176,255}  );
} 
else if (c == '/') { 
   DrawRectangle(x, y, scale, scale, (Color){184, 30, 52,255}  );
} 
else if (c == '~') { 
   DrawRectangle(x, y, scale, scale, (Color){111, 247, 68,255}  );
} 
else if (c == '!') { 
   DrawRectangle(x, y, scale, scale, (Color){82, 180, 193,255}  );
} 
else if (c == '@') { 
   DrawRectangle(x, y, scale, scale, (Color){120, 38, 193,255}  );
} 
else if (c == '#') { 
   DrawRectangle(x, y, scale, scale, (Color){52, 195, 37,255}  );
} 
else if (c == '$') { 
   DrawRectangle(x, y, scale, scale, (Color){150, 149, 183,255}  );
} 
else if (c == '%') { 
   DrawRectangle(x, y, scale, scale, (Color){176, 172, 171,255}  );
} 
else if (c == '^') { 
   DrawRectangle(x, y, scale, scale, (Color){112, 231, 92,255}  );
} 
else if (c == '&') { 
   DrawRectangle(x, y, scale, scale, (Color){222, 193, 233,255}  );
} 
else if (c == '*') { 
   DrawRectangle(x, y, scale, scale, (Color){250, 123, 85,255}  );
} 
else if (c == '(') { 
   DrawRectangle(x, y, scale, scale, (Color){182, 193, 208,255}  );
} 
else if (c == ')') { 
   DrawRectangle(x, y, scale, scale, (Color){193, 129, 224,255}  );
} 
else if (c == '_') { 
   DrawRectangle(x, y, scale, scale, (Color){48, 124, 236,255}  );
} 
else if (c == '+') { 
   DrawRectangle(x, y, scale, scale, (Color){88, 74, 216,255}  );
} 
else if (c == 'Q') { 
   DrawRectangle(x, y, scale, scale, (Color){111, 196, 141,255}  );
} 
else if (c == 'W') { 
   DrawRectangle(x, y, scale, scale, (Color){58, 69, 191,255}  );
} 
else if (c == 'E') { 
   DrawRectangle(x, y, scale, scale, (Color){158, 165, 140,255}  );
} 
else if (c == 'R') { 
   DrawRectangle(x, y, scale, scale, (Color){3, 120, 35,255}  );
} 
else if (c == 'T') { 
   DrawRectangle(x, y, scale, scale, (Color){22, 163, 242,255}  );
} 
else if (c == 'Y') { 
   DrawRectangle(x, y, scale, scale, (Color){175, 13, 225,255}  );
} 
else if (c == 'U') { 
   DrawRectangle(x, y, scale, scale, (Color){17, 121, 241,255}  );
} 
else if (c == 'I') { 
   DrawRectangle(x, y, scale, scale, (Color){96, 125, 32,255}  );
} 
else if (c == 'O') { 
   DrawRectangle(x, y, scale, scale, (Color){73, 248, 183,255}  );
} 
else if (c == 'P') { 
   DrawRectangle(x, y, scale, scale, (Color){199, 225, 212,255}  );
} 
else if (c == '{') { 
   DrawRectangle(x, y, scale, scale, (Color){177, 34, 244,255}  );
} 
else if (c == '}') { 
   DrawRectangle(x, y, scale, scale, (Color){59, 93, 92,255}  );
} 
else if (c == '|') { 
   DrawRectangle(x, y, scale, scale, (Color){215, 172, 99,255}  );
} 
else if (c == 'A') { 
   DrawRectangle(x, y, scale, scale, (Color){169, 14, 141,255}  );
} 
else if (c == 'S') { 
   DrawRectangle(x, y, scale, scale, (Color){5, 17, 54,255}  );
} 
else if (c == 'D') { 
   DrawRectangle(x, y, scale, scale, (Color){174, 64, 143,255}  );
} 
else if (c == 'F') { 
   DrawRectangle(x, y, scale, scale, (Color){184, 180, 254,255}  );
} 
else if (c == 'G') { 
   DrawRectangle(x, y, scale, scale, (Color){66, 221, 217,255}  );
} 
else if (c == 'H') { 
   DrawRectangle(x, y, scale, scale, (Color){227, 97, 195,255}  );
} 
else if (c == 'J') { 
   DrawRectangle(x, y, scale, scale, (Color){161, 224, 104,255}  );
} 
else if (c == 'K') { 
   DrawRectangle(x, y, scale, scale, (Color){171, 143, 174,255}  );
} 
else if (c == 'L') { 
   DrawRectangle(x, y, scale, scale, (Color){183, 81, 61,255}  );
} 
else if (c == ':') { 
   DrawRectangle(x, y, scale, scale, (Color){10, 99, 125,255}  );
} 
else if (c == '"') { 
   DrawRectangle(x, y, scale, scale, (Color){111, 106, 229,255}  );
} 
else if (c == 'Z') { 
   DrawRectangle(x, y, scale, scale, (Color){215, 50, 2,255}  );
} 
else if (c == 'X') { 
   DrawRectangle(x, y, scale, scale, (Color){91, 189, 172,255}  );
} 
else if (c == 'C') { 
   DrawRectangle(x, y, scale, scale, (Color){148, 179, 31,255}  );
} 
else if (c == 'V') { 
   DrawRectangle(x, y, scale, scale, (Color){150, 235, 57,255}  );
} 
else if (c == 'B') { 
   DrawRectangle(x, y, scale, scale, (Color){91, 248, 249,255}  );
} 
else if (c == 'N') { 
   DrawRectangle(x, y, scale, scale, (Color){170, 243, 86,255}  );
} 
else if (c == 'M') { 
   DrawRectangle(x, y, scale, scale, (Color){109, 151, 233,255}  );
} 
else if (c == '<') { 
   DrawRectangle(x, y, scale, scale, (Color){181, 198, 24,255}  );
} 
else if (c == '>') { 
   DrawRectangle(x, y, scale, scale, (Color){54, 8, 73,255}  );
} 
else if (c == '?') { 
   DrawRectangle(x, y, scale, scale, (Color){101, 43, 185,255}  );
}
if (c != '\n') {	
	
printf("[INFO] char: %c - x: %d - y: %d - i: %d\n", c, x, y, i); 
x += scale;
}
else {
printf("[INFO] newline\n");
y += scale;
x = 0 + xtra;
}

		}
EndTextureMode();
  Image image = LoadImageFromTexture(target.texture);
            ImageFlipVertical(&image);
		char pngpath[128] = "";
		strcat(pngpath, argv[1]);
		strcat(pngpath, ".png");
            ExportImage(image, pngpath);
            UnloadImage(image);	
	UnloadRenderTexture(target);
	scale = 20;
	first = false;
	}
	else {


        BeginDrawing();	
	printf("[INFO] Drawing began\n");
            ClearBackground(RAYWHITE);
	printf("[INFO] Background cleared\n");
		x = 0 + xtra;
		y = 0 + ytra;
		printf("[INFO] X & Y set to 0\n");
	if(IsKeyDown(KEY_LEFT)) {
		xtra += 20;
	}
	  if (IsKeyDown(KEY_RIGHT)) {
		xtra = xtra - 20;
	}
	  if (IsKeyDown(KEY_DOWN)) {
		ytra = ytra - 20;
	}
		  if (IsKeyDown(KEY_UP)) {
		ytra += 20;
	}
	if (IsKeyDown(KEY_Z)) {
	scale += 1;
	}
	else if (IsKeyDown(KEY_X)) {
	scale = scale - 1;
	}
		for (int i = 0; i < strlen(main); i++) {
		
		
		
   		 char c = main[i];		
/*		if (c == 'a' ) {
		
 		DrawRectangle(x, y, 20, 20, (Color){ 0, 0, 0, 255 });
		}
		else if (c == 'b') {
		
 		DrawRectangle(x, y, 20, 20, (Color){ 255, 0, 0, 255 });
		} */
		if (c == '`') { 
   DrawRectangle(x, y, scale, scale, (Color){177, 10, 235,255}  );
} 
else if (c == '1') { 
   DrawRectangle(x, y, scale, scale, (Color){164, 248, 225,255}  );
} 
else if (c == '2') { 
   DrawRectangle(x, y, scale, scale, (Color){187, 75, 75,255}  );
} 
else if (c == '3') { 
   DrawRectangle(x, y, scale, scale, (Color){11, 123, 90,255}  );
} 
else if (c == '4') { 
   DrawRectangle(x, y, scale, scale, (Color){202, 119, 54,255}  );
} 
else if (c == '5') { 
   DrawRectangle(x, y, scale, scale, (Color){89, 35, 87,255}  );
} 
else if (c == '6') { 
   DrawRectangle(x, y, scale, scale, (Color){163, 101, 75,255}  );
} 
else if (c == '7') { 
   DrawRectangle(x, y, scale, scale, (Color){56, 172, 190,255}  );
} 
else if (c == '8') { 
   DrawRectangle(x, y, scale, scale, (Color){68, 145, 148,255}  );
} 
else if (c == '9') { 
   DrawRectangle(x, y, scale, scale, (Color){135, 253, 185,255}  );
} 
else if (c == '0') { 
   DrawRectangle(x, y, scale, scale, (Color){88, 33, 16,255}  );
} 
else if (c == '-') { 
   DrawRectangle(x, y, scale, scale, (Color){69, 100, 246,255}  );
} 
else if (c == '=') { 
   DrawRectangle(x, y, scale, scale, (Color){108, 133, 133,255}  );
} 
else if (c == 'q') { 
   DrawRectangle(x, y, scale, scale, (Color){1, 43, 53,255}  );
} 
else if (c == 'w') { 
   DrawRectangle(x, y, scale, scale, (Color){12, 223, 105,255}  );
} 
else if (c == 'e') { 
   DrawRectangle(x, y, scale, scale, (Color){177, 203, 157,255}  );
} 
else if (c == 'r') { 
   DrawRectangle(x, y, scale, scale, (Color){49, 57, 244,255}  );
} 
else if (c == 't') { 
   DrawRectangle(x, y, scale, scale, (Color){200, 253, 223,255}  );
} 
else if (c == 'y') { 
   DrawRectangle(x, y, scale, scale, (Color){88, 113, 153,255}  );
} 
else if (c == 'u') { 
   DrawRectangle(x, y, scale, scale, (Color){190, 94, 238,255}  );
} 
else if (c == 'i') { 
   DrawRectangle(x, y, scale, scale, (Color){126, 42, 110,255}  );
} 
else if (c == 'o') { 
   DrawRectangle(x, y, scale, scale, (Color){128, 42, 236,255}  );
} 
else if (c == 'p') { 
   DrawRectangle(x, y, scale, scale, (Color){87, 184, 127,255}  );
} 
else if (c == '[') { 
   DrawRectangle(x, y, scale, scale, (Color){253, 229, 208,255}  );
} 
else if (c == ']') { 
   DrawRectangle(x, y, scale, scale, (Color){29, 166, 147,255}  );
} 
else if (c == '\\') { 
   DrawRectangle(x, y, scale, scale, (Color){224, 138, 173,255}  );
} 
else if (c == 'a') { 
   DrawRectangle(x, y, scale, scale, (Color){96, 11, 242,255}  );
}
else if (c == 's') { 
   DrawRectangle(x, y, scale, scale, (Color){109, 94, 17,255}  );
}  
else if (c == 'd') { 
   DrawRectangle(x, y, scale, scale, (Color){71, 3, 153,255}  );
} 
else if (c == 'f') { 
   DrawRectangle(x, y, scale, scale, (Color){5, 231, 214,255}  );
} 
else if (c == 'g') { 
   DrawRectangle(x, y, scale, scale, (Color){7, 27, 126,255}  );
} 
else if (c == 'h') { 
   DrawRectangle(x, y, scale, scale, (Color){157, 138, 125,255}  );
} 
else if (c == 'j') { 
   DrawRectangle(x, y, scale, scale, (Color){73, 250, 185,255}  );
} 
else if (c == 'k') { 
   DrawRectangle(x, y, scale, scale, (Color){248, 95, 102,255}  );
} 
else if (c == 'l') { 
   DrawRectangle(x, y, scale, scale, (Color){169, 157, 32,255}  );
} 
else if (c == ';') { 
   DrawRectangle(x, y, scale, scale, (Color){69, 116, 70,255}  );
} 
else if (c == '\'') { 
   DrawRectangle(x, y, scale, scale, (Color){148, 12, 16,255}  );
} 
else if (c == 'z') { 
   DrawRectangle(x, y, scale, scale, (Color){235, 109, 56,255}  );
} 
else if (c == 'x') { 
   DrawRectangle(x, y, scale, scale, (Color){238, 227, 22,255}  );
} 
else if (c == 'c') { 
   DrawRectangle(x, y, scale, scale, (Color){204, 128, 174,255}  );
} 
else if (c == 'v') { 
   DrawRectangle(x, y, scale, scale, (Color){239, 50, 107,255}  );
} 
else if (c == 'b') { 
   DrawRectangle(x, y, scale, scale, (Color){40, 62, 156,255}  );
} 
else if (c == 'n') { 
   DrawRectangle(x, y, scale, scale, (Color){226, 93, 134,255}  );
} 
else if (c == 'm') { 
   DrawRectangle(x, y, scale, scale, (Color){146, 27, 58,255}  );
} 
else if (c == ',') { 
   DrawRectangle(x, y, scale, scale, (Color){164, 132, 217,255}  );
} 
else if (c == '.') { 
   DrawRectangle(x, y, scale, scale, (Color){185, 42, 176,255}  );
} 
else if (c == '/') { 
   DrawRectangle(x, y, scale, scale, (Color){184, 30, 52,255}  );
} 
else if (c == '~') { 
   DrawRectangle(x, y, scale, scale, (Color){111, 247, 68,255}  );
} 
else if (c == '!') { 
   DrawRectangle(x, y, scale, scale, (Color){82, 180, 193,255}  );
} 
else if (c == '@') { 
   DrawRectangle(x, y, scale, scale, (Color){120, 38, 193,255}  );
} 
else if (c == '#') { 
   DrawRectangle(x, y, scale, scale, (Color){52, 195, 37,255}  );
} 
else if (c == '$') { 
   DrawRectangle(x, y, scale, scale, (Color){150, 149, 183,255}  );
} 
else if (c == '%') { 
   DrawRectangle(x, y, scale, scale, (Color){176, 172, 171,255}  );
} 
else if (c == '^') { 
   DrawRectangle(x, y, scale, scale, (Color){112, 231, 92,255}  );
} 
else if (c == '&') { 
   DrawRectangle(x, y, scale, scale, (Color){222, 193, 233,255}  );
} 
else if (c == '*') { 
   DrawRectangle(x, y, scale, scale, (Color){250, 123, 85,255}  );
} 
else if (c == '(') { 
   DrawRectangle(x, y, scale, scale, (Color){182, 193, 208,255}  );
} 
else if (c == ')') { 
   DrawRectangle(x, y, scale, scale, (Color){193, 129, 224,255}  );
} 
else if (c == '_') { 
   DrawRectangle(x, y, scale, scale, (Color){48, 124, 236,255}  );
} 
else if (c == '+') { 
   DrawRectangle(x, y, scale, scale, (Color){88, 74, 216,255}  );
} 
else if (c == 'Q') { 
   DrawRectangle(x, y, scale, scale, (Color){111, 196, 141,255}  );
} 
else if (c == 'W') { 
   DrawRectangle(x, y, scale, scale, (Color){58, 69, 191,255}  );
} 
else if (c == 'E') { 
   DrawRectangle(x, y, scale, scale, (Color){158, 165, 140,255}  );
} 
else if (c == 'R') { 
   DrawRectangle(x, y, scale, scale, (Color){3, 120, 35,255}  );
} 
else if (c == 'T') { 
   DrawRectangle(x, y, scale, scale, (Color){22, 163, 242,255}  );
} 
else if (c == 'Y') { 
   DrawRectangle(x, y, scale, scale, (Color){175, 13, 225,255}  );
} 
else if (c == 'U') { 
   DrawRectangle(x, y, scale, scale, (Color){17, 121, 241,255}  );
} 
else if (c == 'I') { 
   DrawRectangle(x, y, scale, scale, (Color){96, 125, 32,255}  );
} 
else if (c == 'O') { 
   DrawRectangle(x, y, scale, scale, (Color){73, 248, 183,255}  );
} 
else if (c == 'P') { 
   DrawRectangle(x, y, scale, scale, (Color){199, 225, 212,255}  );
} 
else if (c == '{') { 
   DrawRectangle(x, y, scale, scale, (Color){177, 34, 244,255}  );
} 
else if (c == '}') { 
   DrawRectangle(x, y, scale, scale, (Color){59, 93, 92,255}  );
} 
else if (c == '|') { 
   DrawRectangle(x, y, scale, scale, (Color){215, 172, 99,255}  );
} 
else if (c == 'A') { 
   DrawRectangle(x, y, scale, scale, (Color){169, 14, 141,255}  );
} 
else if (c == 'S') { 
   DrawRectangle(x, y, scale, scale, (Color){5, 17, 54,255}  );
} 
else if (c == 'D') { 
   DrawRectangle(x, y, scale, scale, (Color){174, 64, 143,255}  );
} 
else if (c == 'F') { 
   DrawRectangle(x, y, scale, scale, (Color){184, 180, 254,255}  );
} 
else if (c == 'G') { 
   DrawRectangle(x, y, scale, scale, (Color){66, 221, 217,255}  );
} 
else if (c == 'H') { 
   DrawRectangle(x, y, scale, scale, (Color){227, 97, 195,255}  );
} 
else if (c == 'J') { 
   DrawRectangle(x, y, scale, scale, (Color){161, 224, 104,255}  );
} 
else if (c == 'K') { 
   DrawRectangle(x, y, scale, scale, (Color){171, 143, 174,255}  );
} 
else if (c == 'L') { 
   DrawRectangle(x, y, scale, scale, (Color){183, 81, 61,255}  );
} 
else if (c == ':') { 
   DrawRectangle(x, y, scale, scale, (Color){10, 99, 125,255}  );
} 
else if (c == '"') { 
   DrawRectangle(x, y, scale, scale, (Color){111, 106, 229,255}  );
} 
else if (c == 'Z') { 
   DrawRectangle(x, y, scale, scale, (Color){215, 50, 2,255}  );
} 
else if (c == 'X') { 
   DrawRectangle(x, y, scale, scale, (Color){91, 189, 172,255}  );
} 
else if (c == 'C') { 
   DrawRectangle(x, y, scale, scale, (Color){148, 179, 31,255}  );
} 
else if (c == 'V') { 
   DrawRectangle(x, y, scale, scale, (Color){150, 235, 57,255}  );
} 
else if (c == 'B') { 
   DrawRectangle(x, y, scale, scale, (Color){91, 248, 249,255}  );
} 
else if (c == 'N') { 
   DrawRectangle(x, y, scale, scale, (Color){170, 243, 86,255}  );
} 
else if (c == 'M') { 
   DrawRectangle(x, y, scale, scale, (Color){109, 151, 233,255}  );
} 
else if (c == '<') { 
   DrawRectangle(x, y, scale, scale, (Color){181, 198, 24,255}  );
} 
else if (c == '>') { 
   DrawRectangle(x, y, scale, scale, (Color){54, 8, 73,255}  );
} 
else if (c == '?') { 
   DrawRectangle(x, y, scale, scale, (Color){101, 43, 185,255}  );
}
if (c != '\n') {	
	
printf("[INFO] char: %c - x: %d - y: %d - i: %d\n", c, x, y, i); 
x += scale;
}
else {
printf("[INFO] newline\n");
y += scale;
x = 0 + xtra;
}

		}
		

		 
        EndDrawing();
	printf("[INFO] Drawing ended\n");
	}
    }

    CloseWindow();
	printf("[INFO] Window closed\n");
    return 0;
}
